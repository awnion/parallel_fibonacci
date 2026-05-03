use std::marker::PhantomData;

wit_bindgen::generate!({
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

        world runtime-client {
            import runtime;
        }
    "#,
});

use rust_wasm_fib::fib::runtime as wit_runtime;

pub use runtime_macros::{Guest, callable, component, spawn};
pub use wit_runtime::ChildStatus;

pub trait ComponentCall {
    type Output;

    const FUNCTION: &'static str;

    fn encode(self) -> Vec<u8>;
    fn decode(payload: Vec<u8>) -> Result<Self::Output, ChildStatus>;
}

pub struct Task<C: ComponentCall> {
    inner: wit_runtime::Task,
    _call: PhantomData<C>,
}

impl<C: ComponentCall> Task<C> {
    pub async fn join(self) -> Result<C::Output, ChildStatus> {
        let payload = wit_runtime::await_task(self.inner).await?;
        C::decode(payload)
    }
}

pub fn spawn_call<C: ComponentCall>(call: C) -> Task<C> {
    let payload = call.encode();
    let inner = wit_runtime::spawn(C::FUNCTION, &payload);
    Task {
        inner,
        _call: PhantomData,
    }
}

pub fn encode_u64(value: u64) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

pub fn decode_u64(payload: Vec<u8>) -> Result<u64, ChildStatus> {
    let bytes: [u8; 8] = payload.try_into().map_err(|_| ChildStatus::RuntimeError)?;
    Ok(u64::from_le_bytes(bytes))
}
