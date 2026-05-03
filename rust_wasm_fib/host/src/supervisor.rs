use crate::args::{ChildInit, SpawnLinkArgs};
use crate::engine::build_sync_component_engine;
use crate::status::{
    STATUS_BAD_EXPORT, STATUS_OK, STATUS_RUNTIME_ERROR, classify_child_error, status_name,
};
use crate::util::{ensure_path, with_wasmtime_context};
use anyhow::{Result, anyhow};
use std::path::Path;
use std::sync::Arc;
use wasmtime::component::{Component as WasmComponent, Linker as ComponentLinker, ResourceTable};
use wasmtime::{Engine, Store, StoreContextMut};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

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

struct SpawnLinkState {
    wasi: WasiCtx,
    table: ResourceTable,
    child: Arc<ChildRuntime>,
    runtime_function: String,
}

impl SpawnLinkState {
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

impl WasiView for SpawnLinkState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl ChildRuntime {
    fn new(
        engine: &Engine,
        component_path: &Path,
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

    fn spawn_link(&self, arg: u64) -> std::result::Result<u64, (i32, String)> {
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

fn add_spawn_link_runtime_imports(
    linker: &mut ComponentLinker<SpawnLinkState>,
    runtime_import: &str,
    runtime_function: &str,
) -> Result<()> {
    let mut instance = with_wasmtime_context(
        linker.instance(runtime_import),
        format!("failed to create supervisor runtime import instance {runtime_import}"),
    )?;
    let mut function_names = vec![runtime_function];
    if runtime_function == "spawn-link" {
        function_names.push("run-child");
    }

    for function_name in function_names {
        with_wasmtime_context(
            instance.func_wrap(
                function_name,
                |caller: StoreContextMut<'_, SpawnLinkState>, (arg,): (u64,)| {
                    let child = caller.data().child.clone();
                    let runtime_function = caller.data().runtime_function.clone();

                    let result = match child.spawn_link(arg) {
                        Ok(result) => (STATUS_OK, result),
                        Err((status, _error)) => {
                            println!(
                                "runtime.{runtime_function}: linked child status={} ({})",
                                status,
                                status_name(status)
                            );
                            (status, 0)
                        }
                    };

                    Ok((result,))
                },
            ),
            format!("failed to link supervisor runtime function {function_name}"),
        )?;
    }

    Ok(())
}

pub(crate) fn run_spawn_link(args: &SpawnLinkArgs) -> Result<()> {
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

    let mut linker = ComponentLinker::<SpawnLinkState>::new(&engine);
    with_wasmtime_context(
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker),
        "failed to link supervisor WASIp2 imports",
    )?;
    add_spawn_link_runtime_imports(&mut linker, &args.runtime_import, &args.runtime_function)?;

    let mut store = Store::new(
        &engine,
        SpawnLinkState::new(child, args.runtime_function.clone()),
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
        "Supervisor exit status = {} ({})",
        supervisor_status,
        status_name(supervisor_status)
    );
    println!("Spawn attempts = {attempts}");
    if let Some(init) = &args.child_init {
        println!("Child init argument = {}", init.arg);
    }
    println!(
        "Linked child status = {} ({})",
        child_status,
        status_name(child_status)
    );
    if supervisor_status == STATUS_OK {
        println!("Result = {result}");
    } else {
        println!("Runtime survived linked child traps; supervisor reported failure after retries.");
    }

    Ok(())
}
