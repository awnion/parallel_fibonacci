use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};

const STATUS_OK: i32 = 0;
const STATUS_SUPERVISOR_FAILED: i32 = 10;

static LAST_ATTEMPTS: AtomicU32 = AtomicU32::new(0);
static LAST_CHILD_STATUS: AtomicI32 = AtomicI32::new(STATUS_OK);
static LAST_RESULT: AtomicU64 = AtomicU64::new(0);

#[link(wasm_import_module = "runtime")]
unsafe extern "C" {
    #[link_name = "run-fib"]
    fn runtime_run_fib(n: u64, result_ptr: *mut u64) -> i32;
}

#[unsafe(no_mangle)]
pub extern "C" fn supervise_fib(n: u64, max_retries: u32) -> i32 {
    LAST_ATTEMPTS.store(0, Ordering::Relaxed);
    LAST_CHILD_STATUS.store(STATUS_OK, Ordering::Relaxed);
    LAST_RESULT.store(0, Ordering::Relaxed);

    for attempt in 1..=max_retries {
        let mut result = 0;
        let status = unsafe { runtime_run_fib(n, &mut result) };

        LAST_ATTEMPTS.store(attempt, Ordering::Relaxed);
        LAST_CHILD_STATUS.store(status, Ordering::Relaxed);

        if status == STATUS_OK {
            LAST_RESULT.store(result, Ordering::Relaxed);
            return STATUS_OK;
        }
    }

    STATUS_SUPERVISOR_FAILED
}

#[unsafe(no_mangle)]
pub extern "C" fn last_attempts() -> u32 {
    LAST_ATTEMPTS.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn last_child_status() -> i32 {
    LAST_CHILD_STATUS.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn last_result() -> u64 {
    LAST_RESULT.load(Ordering::Relaxed)
}

fn main() {}
