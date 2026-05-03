use std::marker::PhantomData;

wit_bindgen::generate!({
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
});

use rust_wasm_runtime::process::runtime as wit_runtime;

pub use runtime_macros::{
    Guest, callable, component, fail_child_component, fail_supervisor_component, run_child, spawn,
};
pub use wit_runtime::ChildStatus;

pub const STATUS_OK: i32 = 0;
pub const STATUS_STACK_OVERFLOW: i32 = 1;
pub const STATUS_TRAP: i32 = 2;
pub const STATUS_BAD_EXPORT: i32 = 3;
pub const STATUS_RUNTIME_ERROR: i32 = 4;
pub const STATUS_SUPERVISOR_FAILED: i32 = 10;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SupervisorReport {
    pub status: i32,
    pub attempts: u32,
    pub child_status: i32,
    pub result: u64,
}

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
