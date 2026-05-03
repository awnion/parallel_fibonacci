use crate::args::RunArgs;
use crate::codec::{decode_u64, encode_u64};
use crate::engine::build_async_component_engine;
use crate::status::{RunFailure, classify_run_error};
use crate::util::{ensure_path, with_wasmtime_context};
use anyhow::{Result, anyhow};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use wasmtime::component::{
    Accessor, Component as WasmComponent, Linker as ComponentLinker, Resource, ResourceTable,
    ResourceType,
};
use wasmtime::{Engine, Store, StoreContextMut};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

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

pub(crate) async fn run_component(args: &RunArgs) -> Result<u64> {
    ensure_path(&args.component_path, "component")?;
    let engine = build_async_component_engine()?;
    let component = with_wasmtime_context(
        WasmComponent::from_file(&engine, &args.component_path),
        format!("failed to load component {}", args.component_path.display()),
    )?;
    let runtime = ComponentRuntime::new(engine, &component, &args.runtime_import)?;

    runtime.call_export(&args.function, args.arg).await
}
