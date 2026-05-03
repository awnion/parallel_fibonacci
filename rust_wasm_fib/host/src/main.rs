use anyhow::{Result, anyhow, bail};
use std::env;
use std::fmt::Display;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use wasmtime::{Caller, Config, Engine, Instance, Linker, Module, Store, Trap};
use wasmtime_wasi::WasiCtx;
use wasmtime_wasi::p1::WasiP1Ctx;
use wasmtime_wasi_threads::WasiThreadsCtx;

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

struct HostState {
    wasi: WasiP1Ctx,
    threads: Option<Arc<WasiThreadsCtx<HostState>>>,
}

impl HostState {
    fn new(threads: Option<Arc<WasiThreadsCtx<HostState>>>) -> Self {
        let wasi = WasiCtx::builder().inherit_stderr().build_p1();
        Self { wasi, threads }
    }
}

impl Clone for HostState {
    fn clone(&self) -> Self {
        Self::new(self.threads.clone())
    }
}

struct FibArgs {
    n: u64,
    threads: usize,
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
    linker: Arc<Linker<ChildState>>,
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
        .join("wasm32-wasip1-threads")
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
    result.map_err(|error| anyhow!("{context}: {error}"))
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
    let threads = args
        .get(1)
        .and_then(|value| value.parse().ok())
        .filter(|threads| *threads > 0)
        .unwrap_or_else(default_threads);
    let wasm_path = args
        .get(2)
        .map(PathBuf::from)
        .or_else(|| env::var_os("RUST_WASM_FIB_GUEST").map(PathBuf::from))
        .unwrap_or_else(default_fib_guest_path);

    Command::Fib(FibArgs {
        n,
        threads,
        wasm_path,
    })
}

fn build_engine() -> Result<Engine> {
    build_engine_with_stack(None)
}

fn build_engine_with_stack(max_wasm_stack: Option<usize>) -> Result<Engine> {
    let mut config = Config::new();
    config.wasm_threads(true);
    config.shared_memory(true);
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

fn run_wasm_fib(args: &FibArgs) -> Result<u64> {
    let engine = build_engine()?;
    let module = with_wasmtime_context(
        Module::from_file(&engine, &args.wasm_path),
        format!("failed to load guest module {}", args.wasm_path.display()),
    )?;

    let mut linker = Linker::<HostState>::new(&engine);
    with_wasmtime_context(
        wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |state| &mut state.wasi),
        "failed to link WASIp1 imports",
    )?;

    let mut store = Store::new(&engine, HostState::new(None));
    with_wasmtime_context(
        wasmtime_wasi_threads::add_to_linker(&mut linker, &store, &module, |state| {
            state
                .threads
                .as_deref()
                .expect("WASI threads context must be initialized before guest execution")
        }),
        "failed to link wasi-threads imports",
    )?;

    let linker = Arc::new(linker);
    let thread_ctx = Arc::new(with_wasmtime_context(
        WasiThreadsCtx::new(module.clone(), linker.clone(), false),
        "failed to prepare wasi-threads context",
    )?);
    store.data_mut().threads = Some(thread_ctx);

    let instance = with_wasmtime_context(
        linker.instantiate(&mut store, &module),
        "failed to instantiate guest module",
    )?;

    call_guest_start(&mut store, &instance)?;

    let init = with_wasmtime_context(
        instance.get_typed_func::<u32, ()>(&mut store, "init"),
        "guest module does not export init(worker_threads: u32)",
    )?;
    with_wasmtime_context(
        init.call(&mut store, args.threads as u32),
        "guest init failed",
    )?;

    let fib = with_wasmtime_context(
        instance.get_typed_func::<u64, u64>(&mut store, "fib"),
        "guest module does not export fib(n: u64) -> u64",
    )?;
    with_wasmtime_context(fib.call(&mut store, args.n), "guest fib call failed")
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

        let mut linker = Linker::<ChildState>::new(engine);
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

fn write_supervisor_result(
    caller: &mut Caller<'_, FailSupervisorState>,
    result_ptr: u32,
    result: u64,
) -> std::result::Result<(), String> {
    let memory = caller
        .get_export("memory")
        .and_then(|export| export.into_memory())
        .ok_or_else(|| "supervisor module does not export memory".to_string())?;

    memory
        .write(caller, result_ptr as usize, &result.to_le_bytes())
        .map_err(|error| format!("failed to write child result into supervisor memory: {error}"))
}

fn add_supervisor_runtime_imports(linker: &mut Linker<FailSupervisorState>) -> Result<()> {
    with_wasmtime_context(
        linker
            .func_wrap(
                "runtime",
                "run-fib",
                |mut caller: Caller<'_, FailSupervisorState>, n: u64, result_ptr: u32| -> i32 {
                    let child = caller.data().child.clone();

                    match child.run_fib(n) {
                        Ok(result) => {
                            match write_supervisor_result(&mut caller, result_ptr, result) {
                                Ok(()) => CHILD_STATUS_OK,
                                Err(error) => {
                                    println!("runtime.run-fib: {error}");
                                    CHILD_STATUS_RESULT_WRITE_FAILED
                                }
                            }
                        }
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

    let mut linker = Linker::<FailSupervisorState>::new(&engine);
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
                    "guest wasm not found at {}. Build it first with: cargo build -p rust_wasm_fib_guest --target wasm32-wasip1-threads --release",
                    args.wasm_path.display()
                );
            }

            let result = run_wasm_fib(&args)?;
            println!("Fib {} = {}", args.n, result);
            Ok(())
        }
        Command::FailDemo(args) => run_fail_demo(&args),
    }
}
