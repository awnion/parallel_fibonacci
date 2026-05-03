use anyhow::{Result, anyhow, bail};
use std::env;
use std::fmt::Display;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::runtime::Builder;
use tokio::sync::Notify;
use wasmtime::component::{
    Accessor, Component as WasmComponent, Linker as ComponentLinker, Resource, ResourceTable,
    ResourceType,
};
use wasmtime::{
    Caller, Config, Engine, Instance, Linker as CoreLinker, Module, Store, StoreContextMut, Trap,
};
use wasmtime_wasi::p1::WasiP1Ctx;
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

const DEFAULT_N: u64 = 52;
const DEFAULT_RETRIES: u32 = 3;
const DEFAULT_FAIL_BURN_ITERS: u64 = 1_700_000;

const CHILD_STATUS_OK: i32 = 0;
const CHILD_STATUS_STACK_OVERFLOW: i32 = 1;
const CHILD_STATUS_TRAP: i32 = 2;
const CHILD_STATUS_BAD_EXPORT: i32 = 3;
const CHILD_STATUS_RUNTIME_ERROR: i32 = 4;
const CHILD_STATUS_RESULT_WRITE_FAILED: i32 = 5;
const SUPERVISOR_STATUS_FAILED: i32 = 10;

#[derive(Clone, Copy)]
enum FibStatus {
    StackOverflow,
    Trap,
    BadExport,
    RuntimeError,
}

pub struct FibTask {
    result: Arc<FibTaskResult>,
}

mod fib_component {
    wasmtime::component::bindgen!({
        world: "fib-guest",
        inline: r#"
            package rust-wasm-fib:fib;

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

            world fib-guest {
                import runtime;

                export fib: async func(n: u64) -> u64;
            }
        "#,
        imports: {
            "rust-wasm-fib:fib/runtime.await-task": async | trappable | ignore_wit,
            default: trappable,
        },
        with: {
            "rust-wasm-fib:fib/runtime.task": super::FibTask,
        },
    });
}

use fib_component::rust_wasm_fib::fib::runtime::ChildStatus;

struct FibArgs {
    n: u64,
    workers: usize,
    wasm_path: PathBuf,
}

struct FailDemoArgs {
    n: u64,
    retries: u32,
    supervisor_path: PathBuf,
    child_path: PathBuf,
    burn_iters: u64,
}

enum Command {
    Fib(FibArgs),
    FailDemo(FailDemoArgs),
}

struct FibComponentState {
    wasi: WasiCtx,
    table: ResourceTable,
    runtime: Arc<FibRuntime>,
}

impl FibComponentState {
    fn new(runtime: Arc<FibRuntime>) -> Self {
        let wasi = WasiCtx::builder().inherit_stderr().build();
        Self {
            wasi,
            table: ResourceTable::new(),
            runtime,
        }
    }
}

impl WasiView for FibComponentState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

struct ChildState {
    wasi: WasiP1Ctx,
}

impl ChildState {
    fn new() -> Self {
        let wasi = WasiCtx::builder().inherit_stderr().build_p1();
        Self { wasi }
    }
}

struct FailChildRuntime {
    engine: Engine,
    module: Module,
    linker: Arc<CoreLinker<ChildState>>,
    burn_iters: u64,
}

struct FailSupervisorState {
    wasi: WasiP1Ctx,
    child: Arc<FailChildRuntime>,
}

impl FailSupervisorState {
    fn new(child: Arc<FailChildRuntime>) -> Self {
        let wasi = WasiCtx::builder().inherit_stderr().build_p1();
        Self { wasi, child }
    }
}

struct FibTaskResult {
    value: Mutex<Option<std::result::Result<Vec<u8>, FibStatus>>>,
    ready: Notify,
}

impl FibTaskResult {
    fn new() -> Self {
        Self {
            value: Mutex::new(None),
            ready: Notify::new(),
        }
    }

    fn complete(&self, value: std::result::Result<Vec<u8>, FibStatus>) {
        *self.value.lock().expect("fib job result mutex poisoned") = Some(value);
        self.ready.notify_waiters();
    }

    async fn wait(&self) -> std::result::Result<Vec<u8>, FibStatus> {
        loop {
            let ready = self.ready.notified();
            if let Some(value) = self
                .value
                .lock()
                .expect("fib job result mutex poisoned")
                .clone()
            {
                return value;
            }
            ready.await;
        }
    }
}

struct FibRuntime {
    engine: Engine,
    instance_pre: Arc<wasmtime::component::InstancePre<FibComponentState>>,
}

fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(1)
}

fn workspace_root() -> PathBuf {
    let host_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    host_dir
        .parent()
        .expect("host crate should be inside the workspace")
        .to_path_buf()
}

fn default_fib_guest_path() -> PathBuf {
    workspace_root()
        .join("target")
        .join("wasm32-wasip2")
        .join("release")
        .join("rust_wasm_fib_guest.wasm")
}

fn default_fail_supervisor_path() -> PathBuf {
    workspace_root()
        .join("target")
        .join("wasm32-wasip1")
        .join("release")
        .join("rust_wasm_fib_guest_fail_supervisor.wasm")
}

fn default_fail_child_path() -> PathBuf {
    workspace_root()
        .join("target")
        .join("wasm32-wasip1")
        .join("release")
        .join("rust_wasm_fib_guest_fail_child.wasm")
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

fn parse_command() -> Command {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.first().is_some_and(|arg| arg == "fail-demo") {
        let n = args
            .get(1)
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_N);
        let retries = args
            .get(2)
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_RETRIES);
        let supervisor_path = args
            .get(3)
            .map(PathBuf::from)
            .or_else(|| env::var_os("RUST_WASM_FIB_FAIL_SUPERVISOR").map(PathBuf::from))
            .unwrap_or_else(default_fail_supervisor_path);
        let child_path = args
            .get(4)
            .map(PathBuf::from)
            .or_else(|| env::var_os("RUST_WASM_FIB_FAIL_CHILD").map(PathBuf::from))
            .unwrap_or_else(default_fail_child_path);
        let burn_iters = args
            .get(5)
            .and_then(|value| value.parse().ok())
            .or_else(|| {
                env::var("RUST_WASM_FIB_FAIL_BURN_ITERS")
                    .ok()
                    .and_then(|value| value.parse().ok())
            })
            .unwrap_or(DEFAULT_FAIL_BURN_ITERS);

        return Command::FailDemo(FailDemoArgs {
            n,
            retries,
            supervisor_path,
            child_path,
            burn_iters,
        });
    }

    let n = args
        .first()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_N);
    let workers = args
        .get(1)
        .and_then(|value| value.parse().ok())
        .filter(|workers| *workers > 0)
        .unwrap_or_else(default_threads);
    let wasm_path = args
        .get(2)
        .map(PathBuf::from)
        .or_else(|| env::var_os("RUST_WASM_FIB_GUEST").map(PathBuf::from))
        .unwrap_or_else(default_fib_guest_path);

    Command::Fib(FibArgs {
        n,
        workers,
        wasm_path,
    })
}

fn build_component_engine() -> Result<Engine> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);
    config.wasm_component_model_async_builtins(true);
    config.wasm_component_model_async_stackful(true);
    with_wasmtime_context(
        Engine::new(&config),
        "failed to create Wasmtime component engine",
    )
}

fn build_engine_with_stack(max_wasm_stack: Option<usize>) -> Result<Engine> {
    let mut config = Config::new();
    if let Some(max_wasm_stack) = max_wasm_stack {
        config.max_wasm_stack(max_wasm_stack);
    }
    with_wasmtime_context(Engine::new(&config), "failed to create Wasmtime engine")
}

fn call_guest_start<T>(store: &mut Store<T>, instance: &Instance) -> Result<()> {
    if let Some(initialize) = instance.get_func(&mut *store, "_initialize") {
        let initialize = with_wasmtime_context(
            initialize.typed::<(), ()>(&mut *store),
            "guest _initialize export has an unexpected signature",
        )?;
        with_wasmtime_context(initialize.call(&mut *store, ()), "guest _initialize failed")?;
    } else if let Some(start) = instance.get_func(&mut *store, "_start") {
        let start = with_wasmtime_context(
            start.typed::<(), ()>(&mut *store),
            "guest _start export has an unexpected signature",
        )?;
        with_wasmtime_context(start.call(&mut *store, ()), "guest _start failed")?;
    }

    Ok(())
}

fn write_caller_u64<T>(
    caller: &mut Caller<'_, T>,
    result_ptr: u32,
    result: u64,
) -> std::result::Result<(), String> {
    let memory = caller
        .get_export("memory")
        .and_then(|export| export.into_memory())
        .ok_or_else(|| "guest module does not export memory".to_string())?;

    memory
        .write(caller, result_ptr as usize, &result.to_le_bytes())
        .map_err(|error| format!("failed to write u64 into guest memory: {error}"))
}

fn add_fib_component_imports(linker: &mut ComponentLinker<FibComponentState>) -> Result<()> {
    let mut instance = with_wasmtime_context(
        linker.instance("rust-wasm-fib:fib/runtime"),
        "failed to create fib runtime import instance",
    )?;

    with_wasmtime_context(
        instance.resource(
            "task",
            ResourceType::host::<FibTask>(),
            |mut store, rep| -> wasmtime::Result<()> {
                let task = Resource::<FibTask>::new_own(rep);
                store.data_mut().table.delete(task)?;
                Ok(())
            },
        ),
        "failed to link runtime.task resource",
    )?;

    with_wasmtime_context(
        instance.func_wrap(
            "spawn",
            |mut caller: StoreContextMut<'_, FibComponentState>,
             (function, payload): (String, Vec<u8>)| {
                let runtime = caller.data().runtime.clone();
                let result = runtime.spawn_component(function, payload);
                Ok((caller.data_mut().table.push(FibTask { result })?,))
            },
        ),
        "failed to link runtime.spawn import",
    )?;

    with_wasmtime_context(
        instance.func_wrap_concurrent(
            "await-task",
            |caller: &Accessor<FibComponentState>, (task,): (Resource<FibTask>,)| {
                Box::pin(async move {
                    let task = caller.with(|mut access| access.get().table.delete(task))?;
                    let result = task
                        .result
                        .wait()
                        .await
                        .map_err(FibStatus::into_child_status);
                    Ok((result,))
                })
            },
        ),
        "failed to link runtime.await-task import",
    )?;

    Ok(())
}

impl FibRuntime {
    fn new(engine: Engine, component: &WasmComponent) -> Result<Arc<Self>> {
        let mut linker = ComponentLinker::<FibComponentState>::new(&engine);
        with_wasmtime_context(
            wasmtime_wasi::p2::add_to_linker_async(&mut linker),
            "failed to link fib guest WASIp2 imports",
        )?;
        add_fib_component_imports(&mut linker)?;

        let instance_pre = with_wasmtime_context(
            linker.instantiate_pre(component),
            "failed to pre-instantiate fib guest module",
        )?;

        Ok(Arc::new(Self {
            engine,
            instance_pre: Arc::new(instance_pre),
        }))
    }

    fn spawn_component(self: &Arc<Self>, function: String, payload: Vec<u8>) -> Arc<FibTaskResult> {
        let result = Arc::new(FibTaskResult::new());
        let task_result = result.clone();
        let runtime = self.clone();

        tokio::spawn(async move {
            let value = runtime
                .call_component_export(&function, &payload)
                .await
                .map_err(|error| {
                    println!("runtime.spawn: child {function} failed: {error}");
                    classify_fib_error(&error)
                });
            task_result.complete(value);
        });

        result
    }

    async fn call_fib(self: &Arc<Self>, n: u64) -> Result<u64> {
        let payload = self.call_component_export("fib", &encode_u64(n)).await?;
        decode_u64(&payload)
    }

    async fn call_component_export(
        self: &Arc<Self>,
        function: &str,
        payload: &[u8],
    ) -> Result<Vec<u8>> {
        let n = decode_u64(payload)?;
        let mut store = Store::new(&self.engine, FibComponentState::new(self.clone()));
        let instance = with_wasmtime_context(
            self.instance_pre.instantiate_async(&mut store).await,
            "failed to instantiate fib guest process",
        )?;

        let func = instance
            .get_func(&mut store, function)
            .ok_or_else(|| anyhow!("guest component does not export {function}(n: u64) -> u64"))?;
        let func = with_wasmtime_context(
            func.typed::<(u64,), (u64,)>(&store),
            format!("guest export {function} has an unexpected signature"),
        )?;
        let value = with_wasmtime_context(
            store
                .run_concurrent(async |accessor| func.call_concurrent(accessor, (n,)).await)
                .await,
            format!("guest {function} event loop failed"),
        )??;
        Ok(encode_u64(value.0))
    }
}

impl FibStatus {
    fn into_child_status(self) -> ChildStatus {
        match self {
            FibStatus::StackOverflow => ChildStatus::StackOverflow,
            FibStatus::Trap => ChildStatus::Trap,
            FibStatus::BadExport => ChildStatus::BadExport,
            FibStatus::RuntimeError => ChildStatus::RuntimeError,
        }
    }
}

fn classify_fib_error(error: &anyhow::Error) -> FibStatus {
    if matches!(error.downcast_ref::<Trap>(), Some(Trap::StackOverflow)) {
        return FibStatus::StackOverflow;
    }

    let message = error.to_string().to_ascii_lowercase();
    if message.contains("call stack exhausted") || message.contains("stack overflow") {
        FibStatus::StackOverflow
    } else if message.contains("does not export") {
        FibStatus::BadExport
    } else if error.downcast_ref::<wasmtime::Error>().is_some() {
        FibStatus::Trap
    } else {
        FibStatus::RuntimeError
    }
}

async fn run_wasm_fib(args: &FibArgs) -> Result<u64> {
    let engine = build_component_engine()?;
    let component = with_wasmtime_context(
        WasmComponent::from_file(&engine, &args.wasm_path),
        format!(
            "failed to load guest component {}",
            args.wasm_path.display()
        ),
    )?;
    let runtime = FibRuntime::new(engine, &component)?;

    runtime.call_fib(args.n).await
}

fn child_status_name(status: i32) -> &'static str {
    match status {
        CHILD_STATUS_OK => "ok",
        CHILD_STATUS_STACK_OVERFLOW => "stack-overflow",
        CHILD_STATUS_TRAP => "trap",
        CHILD_STATUS_BAD_EXPORT => "bad-export",
        CHILD_STATUS_RUNTIME_ERROR => "runtime-error",
        CHILD_STATUS_RESULT_WRITE_FAILED => "result-write-failed",
        SUPERVISOR_STATUS_FAILED => "supervisor-failed",
        _ => "unknown",
    }
}

fn classify_child_error(error: &wasmtime::Error) -> i32 {
    if matches!(error.downcast_ref::<Trap>(), Some(Trap::StackOverflow)) {
        return CHILD_STATUS_STACK_OVERFLOW;
    }

    let message = error.to_string().to_ascii_lowercase();
    if message.contains("call stack exhausted") || message.contains("stack overflow") {
        CHILD_STATUS_STACK_OVERFLOW
    } else {
        CHILD_STATUS_TRAP
    }
}

impl FailChildRuntime {
    fn new(engine: &Engine, child_path: &PathBuf, burn_iters: u64) -> Result<Self> {
        let module = with_wasmtime_context(
            Module::from_file(engine, child_path),
            format!("failed to load fail child module {}", child_path.display()),
        )?;

        let mut linker = CoreLinker::<ChildState>::new(engine);
        with_wasmtime_context(
            wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |state| &mut state.wasi),
            "failed to link fail child WASIp1 imports",
        )?;

        Ok(Self {
            engine: engine.clone(),
            module,
            linker: Arc::new(linker),
            burn_iters,
        })
    }

    fn run_fib(&self, n: u64) -> std::result::Result<u64, (i32, String)> {
        let mut store = Store::new(&self.engine, ChildState::new());
        let instance = self
            .linker
            .instantiate(&mut store, &self.module)
            .map_err(|error| {
                (
                    CHILD_STATUS_RUNTIME_ERROR,
                    format!("failed to instantiate fail child: {error}"),
                )
            })?;

        call_guest_start(&mut store, &instance).map_err(|error| {
            (
                CHILD_STATUS_RUNTIME_ERROR,
                format!("fail child startup failed: {error}"),
            )
        })?;

        if let Some(init) = instance.get_func(&mut store, "init") {
            let init = init.typed::<u64, ()>(&mut store).map_err(|error| {
                (
                    CHILD_STATUS_BAD_EXPORT,
                    format!("fail child init export has invalid signature: {error}"),
                )
            })?;
            init.call(&mut store, self.burn_iters).map_err(|error| {
                (
                    CHILD_STATUS_RUNTIME_ERROR,
                    format!("fail child init failed: {error}"),
                )
            })?;
        }

        let fib = instance
            .get_typed_func::<u64, u64>(&mut store, "fib")
            .map_err(|error| {
                (
                    CHILD_STATUS_BAD_EXPORT,
                    format!("fail child fib export is missing or invalid: {error}"),
                )
            })?;

        fib.call(&mut store, n).map_err(|error| {
            (
                classify_child_error(&error),
                format!("fail child trapped while running fib({n}): {error}"),
            )
        })
    }
}

fn add_supervisor_runtime_imports(linker: &mut CoreLinker<FailSupervisorState>) -> Result<()> {
    with_wasmtime_context(
        linker
            .func_wrap(
                "runtime",
                "run-fib",
                |mut caller: Caller<'_, FailSupervisorState>, n: u64, result_ptr: u32| -> i32 {
                    let child = caller.data().child.clone();

                    match child.run_fib(n) {
                        Ok(result) => match write_caller_u64(&mut caller, result_ptr, result) {
                            Ok(()) => CHILD_STATUS_OK,
                            Err(error) => {
                                println!("runtime.run-fib: {error}");
                                CHILD_STATUS_RESULT_WRITE_FAILED
                            }
                        },
                        Err((status, _error)) => {
                            println!(
                                "runtime.run-fib: child status={} ({})",
                                status,
                                child_status_name(status)
                            );
                            status
                        }
                    }
                },
            )
            .map(|_| ()),
        "failed to link supervisor runtime imports",
    )
}

fn run_fail_demo(args: &FailDemoArgs) -> Result<()> {
    if !args.supervisor_path.exists() {
        bail!(
            "fail supervisor wasm not found at {}. Build it first with: cargo build -p rust_wasm_fib_guest_fail_supervisor --target wasm32-wasip1 --release",
            args.supervisor_path.display()
        );
    }
    if !args.child_path.exists() {
        bail!(
            "fail child wasm not found at {}. Build it first with: cargo build -p rust_wasm_fib_guest_fail_child --target wasm32-wasip1 --release",
            args.child_path.display()
        );
    }

    let engine = build_engine_with_stack(Some(64 * 1024))?;
    let supervisor_module = with_wasmtime_context(
        Module::from_file(&engine, &args.supervisor_path),
        format!(
            "failed to load fail supervisor module {}",
            args.supervisor_path.display()
        ),
    )?;
    let child = Arc::new(FailChildRuntime::new(
        &engine,
        &args.child_path,
        args.burn_iters,
    )?);

    let mut linker = CoreLinker::<FailSupervisorState>::new(&engine);
    with_wasmtime_context(
        wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |state| &mut state.wasi),
        "failed to link fail supervisor WASIp1 imports",
    )?;
    add_supervisor_runtime_imports(&mut linker)?;

    let mut store = Store::new(&engine, FailSupervisorState::new(child));
    let instance = with_wasmtime_context(
        linker.instantiate(&mut store, &supervisor_module),
        "failed to instantiate fail supervisor module",
    )?;
    call_guest_start(&mut store, &instance)?;

    let supervise = with_wasmtime_context(
        instance.get_typed_func::<(u64, u32), i32>(&mut store, "supervise_fib"),
        "supervisor module does not export supervise_fib(n: u64, retries: u32) -> i32",
    )?;
    let supervisor_status = with_wasmtime_context(
        supervise.call(&mut store, (args.n, args.retries)),
        "supervisor failed while running supervise_fib",
    )?;

    let last_attempts = with_wasmtime_context(
        instance.get_typed_func::<(), u32>(&mut store, "last_attempts"),
        "supervisor module does not export last_attempts() -> u32",
    )?;
    let attempts = with_wasmtime_context(
        last_attempts.call(&mut store, ()),
        "failed to read supervisor attempt count",
    )?;

    let last_child_status = with_wasmtime_context(
        instance.get_typed_func::<(), i32>(&mut store, "last_child_status"),
        "supervisor module does not export last_child_status() -> i32",
    )?;
    let child_status = with_wasmtime_context(
        last_child_status.call(&mut store, ()),
        "failed to read supervisor child status",
    )?;

    let last_result = with_wasmtime_context(
        instance.get_typed_func::<(), u64>(&mut store, "last_result"),
        "supervisor module does not export last_result() -> u64",
    )?;
    let result = with_wasmtime_context(
        last_result.call(&mut store, ()),
        "failed to read supervisor result",
    )?;

    println!(
        "Supervisor status = {} ({})",
        supervisor_status,
        child_status_name(supervisor_status)
    );
    println!("Supervisor attempts = {attempts}");
    println!("Child burn iterations/frame = {}", args.burn_iters);
    println!(
        "Last child status = {} ({})",
        child_status,
        child_status_name(child_status)
    );
    if supervisor_status == CHILD_STATUS_OK {
        println!("Fib {} = {}", args.n, result);
    } else {
        println!("Runtime survived child traps; supervisor reported failure after retries.");
    }

    Ok(())
}

fn main() -> Result<()> {
    match parse_command() {
        Command::Fib(args) => {
            if !args.wasm_path.exists() {
                bail!(
                    "guest component not found at {}. Build it first with: cargo build -p rust_wasm_fib_guest --target wasm32-wasip2 --release",
                    args.wasm_path.display()
                );
            }

            let runtime = Builder::new_multi_thread()
                .worker_threads(args.workers)
                .enable_all()
                .build()?;
            let result = runtime.block_on(run_wasm_fib(&args))?;
            println!("Fib {} = {}", args.n, result);
            Ok(())
        }
        Command::FailDemo(args) => run_fail_demo(&args),
    }
}
