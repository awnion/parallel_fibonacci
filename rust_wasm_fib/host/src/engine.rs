use crate::util::with_wasmtime_context;
use anyhow::Result;
use wasmtime::{Config, Engine};

pub(crate) fn build_async_component_engine() -> Result<Engine> {
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

pub(crate) fn build_sync_component_engine(max_wasm_stack: Option<usize>) -> Result<Engine> {
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
