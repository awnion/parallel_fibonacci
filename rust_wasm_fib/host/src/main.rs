use anyhow::{Result, anyhow, bail};
use std::env;
use std::fmt::Display;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::runtime::Builder;
use tokio::sync::Notify;
use wasmtime::component::{
    Accessor, Component as WasmComponent, Linker as ComponentLinker, Resource, ResourceTable,
    ResourceType,
};
use wasmtime::{Config, Engine, Store, StoreContextMut, Trap};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

const DEFAULT_RUNTIME_IMPORT: &str = "rust-wasm-runtime:process/runtime";
const DEFAULT_SUPERVISOR_IMPORT: &str = "rust-wasm-runtime:supervisor/runtime";
const DEFAULT_SUPERVISOR_IMPORT_FUNCTION: &str = "run-child";
const NO_INIT: &str = "-";

const STATUS_OK: i32 = 0;
const STATUS_STACK_OVERFLOW: i32 = 1;
const STATUS_TRAP: i32 = 2;
const STATUS_BAD_EXPORT: i32 = 3;
const STATUS_RUNTIME_ERROR: i32 = 4;
const STATUS_SUPERVISOR_FAILED: i32 = 10;

#[derive(Clone, Copy)]
enum RunFailure {
    StackOverflow,
    Trap,
    BadExport,
    RuntimeError,
}

pub struct RuntimeTask {
    result: Arc<TaskResult>,
}

mod runtime_component {
    wasmtime::component::bindgen!({
        world: "runtime-client",
        inline: r#"
            package rust-wasm-runtime:process;

            interface runtime {
                resource task;

                enum child-status {
                    stack-overflow,
                    trap,
                    bad-export,
                    runtime-error,
                }

                spawn: func(function: string, payload: list<u8>) -> task;
                await-task: async func(task: task) -> result<list<u8>, child-status>;
            }

            world runtime-client {
                import runtime;
            }
        "#,
        imports: {
            "rust-wasm-runtime:process/runtime.await-task": async | trappable | ignore_wit,
            default: trappable,
        },
        with: {
            "rust-wasm-runtime:process/runtime.task": super::RuntimeTask,
        },
    });
}

use runtime_component::rust_wasm_runtime::process::runtime::ChildStatus;

struct RunArgs {
    component_path: PathBuf,
    function: String,
    arg: u64,
    workers: usize,
    runtime_import: String,
}

#[derive(Clone)]
struct ChildInit {
    function: String,
    arg: u64,
}

struct SuperviseArgs {
    supervisor_path: PathBuf,
    supervisor_function: String,
    child_path: PathBuf,
    child_function: String,
    arg: u64,
    retries: u32,
    child_init: Option<ChildInit>,
    runtime_import: String,
    runtime_function: String,
}

enum Command {
    Run(RunArgs),
    Supervise(SuperviseArgs),
}

struct RuntimeComponentState {
    wasi: WasiCtx,
    table: ResourceTable,
    runtime: Arc<ComponentRuntime>,
}

impl RuntimeComponentState {
    fn new(runtime: Arc<ComponentRuntime>) -> Self {
        let wasi = WasiCtx::builder().inherit_stderr().build();
        Self {
            wasi,
            table: ResourceTable::new(),
            runtime,
        }
    }
}

impl WasiView for RuntimeComponentState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

struct PlainComponentState {
    wasi: WasiCtx,
    table: ResourceTable,
}

impl PlainComponentState {
    fn new() -> Self {
        let wasi = WasiCtx::builder().inherit_stderr().build();
        Self {
            wasi,
            table: ResourceTable::new(),
        }
    }
}

impl WasiView for PlainComponentState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

struct ChildRuntime {
    engine: Engine,
    component: WasmComponent,
    linker: Arc<ComponentLinker<PlainComponentState>>,
    function: String,
    init: Option<ChildInit>,
}

struct SupervisorState {
    wasi: WasiCtx,
    table: ResourceTable,
    child: Arc<ChildRuntime>,
    runtime_function: String,
}

impl SupervisorState {
    fn new(child: Arc<ChildRuntime>, runtime_function: String) -> Self {
        let wasi = WasiCtx::builder().inherit_stderr().build();
        Self {
            wasi,
            table: ResourceTable::new(),
            child,
            runtime_function,
        }
    }
}

impl WasiView for SupervisorState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

struct TaskResult {
    value: Mutex<Option<std::result::Result<Vec<u8>, RunFailure>>>,
    ready: Notify,
}

impl TaskResult {
    fn new() -> Self {
        Self {
            value: Mutex::new(None),
            ready: Notify::new(),
        }
    }

    fn complete(&self, value: std::result::Result<Vec<u8>, RunFailure>) {
        *self.value.lock().expect("task result mutex poisoned") = Some(value);
        self.ready.notify_waiters();
    }

    async fn wait(&self) -> std::result::Result<Vec<u8>, RunFailure> {
        loop {
            let ready = self.ready.notified();
            if let Some(value) = self
                .value
                .lock()
                .expect("task result mutex poisoned")
                .clone()
            {
                return value;
            }
            ready.await;
        }
    }
}

struct ComponentRuntime {
    engine: Engine,
    instance_pre: Arc<wasmtime::component::InstancePre<RuntimeComponentState>>,
}

fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(1)
}

fn with_wasmtime_context<T>(
    result: std::result::Result<T, wasmtime::Error>,
    context: impl Display,
) -> Result<T> {
    result.map_err(|error| anyhow!("{context}: {error:?}"))
}

fn encode_u64(value: u64) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

fn decode_u64(payload: &[u8]) -> Result<u64> {
    let bytes: [u8; 8] = payload.try_into().map_err(|_| {
        anyhow!(
            "expected an 8-byte u64 payload, got {} bytes",
            payload.len()
        )
    })?;
    Ok(u64::from_le_bytes(bytes))
}

fn parse_command() -> Result<Command> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run-u64") => parse_run_command(&args[1..]),
        Some("supervise-u64") => parse_supervise_command(&args[1..]),
        _ => bail!(
            "usage:\n  run-u64 <component> <function> <arg> [workers] [runtime-import]\n  supervise-u64 <supervisor> <supervisor-function> <child> <child-function> <arg> <retries> <child-init-function|-> <child-init-arg> [runtime-import] [runtime-function]"
        ),
    }
}

fn parse_run_command(args: &[String]) -> Result<Command> {
    let component_path = arg_path(args, 0, "component")?;
    let function = arg_string(args, 1, "function")?;
    let arg = arg_parse(args, 2, "arg")?;
    let workers = args
        .get(3)
        .map(|value| parse_named(value, "workers"))
        .transpose()?
        .filter(|workers| *workers > 0)
        .unwrap_or_else(default_threads);
    let runtime_import = args
        .get(4)
        .cloned()
        .unwrap_or_else(|| DEFAULT_RUNTIME_IMPORT.to_string());

    Ok(Command::Run(RunArgs {
        component_path,
        function,
        arg,
        workers,
        runtime_import,
    }))
}

fn parse_supervise_command(args: &[String]) -> Result<Command> {
    let supervisor_path = arg_path(args, 0, "supervisor")?;
    let supervisor_function = arg_string(args, 1, "supervisor-function")?;
    let child_path = arg_path(args, 2, "child")?;
    let child_function = arg_string(args, 3, "child-function")?;
    let arg = arg_parse(args, 4, "arg")?;
    let retries = arg_parse(args, 5, "retries")?;
    let init_function = arg_string(args, 6, "child-init-function")?;
    let init_arg = arg_parse(args, 7, "child-init-arg")?;
    let runtime_import = args
        .get(8)
        .cloned()
        .unwrap_or_else(|| DEFAULT_SUPERVISOR_IMPORT.to_string());
    let runtime_function = args
        .get(9)
        .cloned()
        .unwrap_or_else(|| DEFAULT_SUPERVISOR_IMPORT_FUNCTION.to_string());
    let child_init = (init_function != NO_INIT).then_some(ChildInit {
        function: init_function,
        arg: init_arg,
    });

    Ok(Command::Supervise(SuperviseArgs {
        supervisor_path,
        supervisor_function,
        child_path,
        child_function,
        arg,
        retries,
        child_init,
        runtime_import,
        runtime_function,
    }))
}

fn arg_string(args: &[String], index: usize, name: &str) -> Result<String> {
    args.get(index)
        .cloned()
        .ok_or_else(|| anyhow!("missing argument: {name}"))
}

fn arg_path(args: &[String], index: usize, name: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(arg_string(args, index, name)?))
}

fn arg_parse<T>(args: &[String], index: usize, name: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: Display,
{
    parse_named(
        args.get(index)
            .ok_or_else(|| anyhow!("missing argument: {name}"))?,
        name,
    )
}

fn parse_named<T>(value: &str, name: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: Display,
{
    value
        .parse()
        .map_err(|error| anyhow!("invalid {name} value {value:?}: {error}"))
}

fn build_async_component_engine() -> Result<Engine> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);
    config.wasm_component_model_async_builtins(true);
    config.wasm_component_model_async_stackful(true);
    with_wasmtime_context(
        Engine::new(&config),
        "failed to create Wasmtime async component engine",
    )
}

fn build_sync_component_engine(max_wasm_stack: Option<usize>) -> Result<Engine> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    if let Some(max_wasm_stack) = max_wasm_stack {
        config.max_wasm_stack(max_wasm_stack);
    }
    with_wasmtime_context(
        Engine::new(&config),
        "failed to create Wasmtime sync component engine",
    )
}

fn add_runtime_imports(
    linker: &mut ComponentLinker<RuntimeComponentState>,
    runtime_import: &str,
) -> Result<()> {
    let mut instance = with_wasmtime_context(
        linker.instance(runtime_import),
        format!("failed to create runtime import instance {runtime_import}"),
    )?;

    with_wasmtime_context(
        instance.resource(
            "task",
            ResourceType::host::<RuntimeTask>(),
            |mut store, rep| -> wasmtime::Result<()> {
                let task = Resource::<RuntimeTask>::new_own(rep);
                store.data_mut().table.delete(task)?;
                Ok(())
            },
        ),
        "failed to link runtime.task resource",
    )?;

    with_wasmtime_context(
        instance.func_wrap(
            "spawn",
            |mut caller: StoreContextMut<'_, RuntimeComponentState>,
             (function, payload): (String, Vec<u8>)| {
                let runtime = caller.data().runtime.clone();
                let result = runtime.spawn_component(function, payload);
                Ok((caller.data_mut().table.push(RuntimeTask { result })?,))
            },
        ),
        "failed to link runtime.spawn import",
    )?;

    with_wasmtime_context(
        instance.func_wrap_concurrent(
            "await-task",
            |caller: &Accessor<RuntimeComponentState>, (task,): (Resource<RuntimeTask>,)| {
                Box::pin(async move {
                    let task = caller.with(|mut access| access.get().table.delete(task))?;
                    let result = task
                        .result
                        .wait()
                        .await
                        .map_err(RunFailure::into_child_status);
                    Ok((result,))
                })
            },
        ),
        "failed to link runtime.await-task import",
    )?;

    Ok(())
}

impl ComponentRuntime {
    fn new(engine: Engine, component: &WasmComponent, runtime_import: &str) -> Result<Arc<Self>> {
        let mut linker = ComponentLinker::<RuntimeComponentState>::new(&engine);
        with_wasmtime_context(
            wasmtime_wasi::p2::add_to_linker_async(&mut linker),
            "failed to link component WASIp2 imports",
        )?;
        add_runtime_imports(&mut linker, runtime_import)?;

        let instance_pre = with_wasmtime_context(
            linker.instantiate_pre(component),
            "failed to pre-instantiate component",
        )?;

        Ok(Arc::new(Self {
            engine,
            instance_pre: Arc::new(instance_pre),
        }))
    }

    fn spawn_component(self: &Arc<Self>, function: String, payload: Vec<u8>) -> Arc<TaskResult> {
        let result = Arc::new(TaskResult::new());
        let task_result = result.clone();
        let runtime = self.clone();

        tokio::spawn(async move {
            let value = runtime
                .call_component_export(&function, &payload)
                .await
                .map_err(|error| {
                    println!("runtime.spawn: child {function} failed: {error}");
                    classify_run_error(&error)
                });
            task_result.complete(value);
        });

        result
    }

    async fn call_export(self: &Arc<Self>, function: &str, arg: u64) -> Result<u64> {
        let payload = self
            .call_component_export(function, &encode_u64(arg))
            .await?;
        decode_u64(&payload)
    }

    async fn call_component_export(
        self: &Arc<Self>,
        function: &str,
        payload: &[u8],
    ) -> Result<Vec<u8>> {
        let arg = decode_u64(payload)?;
        let mut store = Store::new(&self.engine, RuntimeComponentState::new(self.clone()));
        let instance = with_wasmtime_context(
            self.instance_pre.instantiate_async(&mut store).await,
            "failed to instantiate component process",
        )?;

        let func = instance
            .get_func(&mut store, function)
            .ok_or_else(|| anyhow!("component does not export {function}(u64) -> u64"))?;
        let func = with_wasmtime_context(
            func.typed::<(u64,), (u64,)>(&store),
            format!("component export {function} has an unexpected signature"),
        )?;
        let value = with_wasmtime_context(
            store
                .run_concurrent(async |accessor| func.call_concurrent(accessor, (arg,)).await)
                .await,
            format!("component {function} event loop failed"),
        )??;
        Ok(encode_u64(value.0))
    }
}

impl RunFailure {
    fn into_child_status(self) -> ChildStatus {
        match self {
            RunFailure::StackOverflow => ChildStatus::StackOverflow,
            RunFailure::Trap => ChildStatus::Trap,
            RunFailure::BadExport => ChildStatus::BadExport,
            RunFailure::RuntimeError => ChildStatus::RuntimeError,
        }
    }
}

fn classify_run_error(error: &anyhow::Error) -> RunFailure {
    if matches!(error.downcast_ref::<Trap>(), Some(Trap::StackOverflow)) {
        return RunFailure::StackOverflow;
    }

    let message = error.to_string().to_ascii_lowercase();
    if message.contains("call stack exhausted") || message.contains("stack overflow") {
        RunFailure::StackOverflow
    } else if message.contains("does not export") {
        RunFailure::BadExport
    } else if error.downcast_ref::<wasmtime::Error>().is_some() {
        RunFailure::Trap
    } else {
        RunFailure::RuntimeError
    }
}

async fn run_component(args: &RunArgs) -> Result<u64> {
    ensure_path(&args.component_path, "component")?;
    let engine = build_async_component_engine()?;
    let component = with_wasmtime_context(
        WasmComponent::from_file(&engine, &args.component_path),
        format!("failed to load component {}", args.component_path.display()),
    )?;
    let runtime = ComponentRuntime::new(engine, &component, &args.runtime_import)?;

    runtime.call_export(&args.function, args.arg).await
}

fn status_name(status: i32) -> &'static str {
    match status {
        STATUS_OK => "ok",
        STATUS_STACK_OVERFLOW => "stack-overflow",
        STATUS_TRAP => "trap",
        STATUS_BAD_EXPORT => "bad-export",
        STATUS_RUNTIME_ERROR => "runtime-error",
        STATUS_SUPERVISOR_FAILED => "supervisor-failed",
        _ => "unknown",
    }
}

fn classify_child_error(error: &wasmtime::Error) -> i32 {
    if matches!(error.downcast_ref::<Trap>(), Some(Trap::StackOverflow)) {
        return STATUS_STACK_OVERFLOW;
    }

    let message = error.to_string().to_ascii_lowercase();
    if message.contains("call stack exhausted") || message.contains("stack overflow") {
        STATUS_STACK_OVERFLOW
    } else {
        STATUS_TRAP
    }
}

impl ChildRuntime {
    fn new(
        engine: &Engine,
        component_path: &PathBuf,
        function: String,
        init: Option<ChildInit>,
    ) -> Result<Self> {
        let component = with_wasmtime_context(
            WasmComponent::from_file(engine, component_path),
            format!(
                "failed to load child component {}",
                component_path.display()
            ),
        )?;

        let mut linker = ComponentLinker::<PlainComponentState>::new(engine);
        with_wasmtime_context(
            wasmtime_wasi::p2::add_to_linker_sync(&mut linker),
            "failed to link child WASIp2 imports",
        )?;

        Ok(Self {
            engine: engine.clone(),
            component,
            linker: Arc::new(linker),
            function,
            init,
        })
    }

    fn run_child(&self, arg: u64) -> std::result::Result<u64, (i32, String)> {
        let mut store = Store::new(&self.engine, PlainComponentState::new());
        let instance = self
            .linker
            .instantiate(&mut store, &self.component)
            .map_err(|error| {
                (
                    STATUS_RUNTIME_ERROR,
                    format!("failed to instantiate child component: {error}"),
                )
            })?;

        if let Some(init) = &self.init {
            let init_func = instance
                .get_func(&mut store, &init.function)
                .ok_or_else(|| {
                    (
                        STATUS_BAD_EXPORT,
                        format!("child component does not export {}(u64)", init.function),
                    )
                })?;
            let init_func = init_func.typed::<(u64,), ()>(&store).map_err(|error| {
                (
                    STATUS_BAD_EXPORT,
                    format!(
                        "child init export {} has invalid signature: {error}",
                        init.function
                    ),
                )
            })?;
            init_func.call(&mut store, (init.arg,)).map_err(|error| {
                (
                    STATUS_RUNTIME_ERROR,
                    format!("child init {} failed: {error}", init.function),
                )
            })?;
        }

        let func = instance
            .get_func(&mut store, &self.function)
            .ok_or_else(|| {
                (
                    STATUS_BAD_EXPORT,
                    format!(
                        "child component does not export {}(u64) -> u64",
                        self.function
                    ),
                )
            })?;
        let func = func.typed::<(u64,), (u64,)>(&store).map_err(|error| {
            (
                STATUS_BAD_EXPORT,
                format!(
                    "child export {} has invalid signature: {error}",
                    self.function
                ),
            )
        })?;

        func.call(&mut store, (arg,))
            .map(|result| result.0)
            .map_err(|error| {
                (
                    classify_child_error(&error),
                    format!(
                        "child trapped while running {}({arg}): {error}",
                        self.function
                    ),
                )
            })
    }
}

fn add_supervisor_runtime_imports(
    linker: &mut ComponentLinker<SupervisorState>,
    runtime_import: &str,
    runtime_function: &str,
) -> Result<()> {
    let mut instance = with_wasmtime_context(
        linker.instance(runtime_import),
        format!("failed to create supervisor runtime import instance {runtime_import}"),
    )?;
    let runtime_function_name = runtime_function.to_string();

    with_wasmtime_context(
        instance.func_wrap(
            runtime_function,
            |caller: StoreContextMut<'_, SupervisorState>, (arg,): (u64,)| {
                let child = caller.data().child.clone();
                let runtime_function = caller.data().runtime_function.clone();

                let result = match child.run_child(arg) {
                    Ok(result) => (STATUS_OK, result),
                    Err((status, _error)) => {
                        println!(
                            "runtime.{runtime_function}: child status={} ({})",
                            status,
                            status_name(status)
                        );
                        (status, 0)
                    }
                };

                Ok((result,))
            },
        ),
        format!("failed to link supervisor runtime function {runtime_function_name}"),
    )
}

fn run_supervisor(args: &SuperviseArgs) -> Result<()> {
    ensure_path(&args.supervisor_path, "supervisor component")?;
    ensure_path(&args.child_path, "child component")?;

    let engine = build_sync_component_engine(Some(64 * 1024))?;
    let supervisor_component = with_wasmtime_context(
        WasmComponent::from_file(&engine, &args.supervisor_path),
        format!(
            "failed to load supervisor component {}",
            args.supervisor_path.display()
        ),
    )?;
    let child = Arc::new(ChildRuntime::new(
        &engine,
        &args.child_path,
        args.child_function.clone(),
        args.child_init.clone(),
    )?);

    let mut linker = ComponentLinker::<SupervisorState>::new(&engine);
    with_wasmtime_context(
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker),
        "failed to link supervisor WASIp2 imports",
    )?;
    add_supervisor_runtime_imports(&mut linker, &args.runtime_import, &args.runtime_function)?;

    let mut store = Store::new(
        &engine,
        SupervisorState::new(child, args.runtime_function.clone()),
    );
    let instance = with_wasmtime_context(
        linker.instantiate(&mut store, &supervisor_component),
        "failed to instantiate supervisor component",
    )?;

    let supervise = with_wasmtime_context(
        instance
            .get_func(&mut store, &args.supervisor_function)
            .ok_or_else(|| {
                anyhow!(
                    "supervisor component does not export {}",
                    args.supervisor_function
                )
            })?
            .typed::<(u64, u32), ((i32, u32, i32, u64),)>(&store),
        format!(
            "supervisor export {} has invalid signature",
            args.supervisor_function
        ),
    )?;
    let ((supervisor_status, attempts, child_status, result),) = with_wasmtime_context(
        supervise.call(&mut store, (args.arg, args.retries)),
        format!(
            "supervisor failed while running {}",
            args.supervisor_function
        ),
    )?;

    println!(
        "Supervisor status = {} ({})",
        supervisor_status,
        status_name(supervisor_status)
    );
    println!("Supervisor attempts = {attempts}");
    if let Some(init) = &args.child_init {
        println!("Child init argument = {}", init.arg);
    }
    println!(
        "Last child status = {} ({})",
        child_status,
        status_name(child_status)
    );
    if supervisor_status == STATUS_OK {
        println!("Result = {result}");
    } else {
        println!("Runtime survived child traps; supervisor reported failure after retries.");
    }

    Ok(())
}

fn ensure_path(path: &Path, label: &str) -> Result<()> {
    if !path.exists() {
        bail!("{label} not found at {}", path.display());
    }
    Ok(())
}

fn main() -> Result<()> {
    match parse_command()? {
        Command::Run(args) => {
            let runtime = Builder::new_multi_thread()
                .worker_threads(args.workers)
                .enable_all()
                .build()?;
            let result = runtime.block_on(run_component(&args))?;
            println!("{result}");
            Ok(())
        }
        Command::Supervise(args) => run_supervisor(&args),
    }
}
