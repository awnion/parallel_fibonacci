use wasmtime::Trap;

pub(crate) const STATUS_OK: i32 = 0;
pub(crate) const STATUS_STACK_OVERFLOW: i32 = 1;
pub(crate) const STATUS_TRAP: i32 = 2;
pub(crate) const STATUS_BAD_EXPORT: i32 = 3;
pub(crate) const STATUS_RUNTIME_ERROR: i32 = 4;
pub(crate) const STATUS_SUPERVISOR_FAILED: i32 = 10;

#[derive(Clone, Copy)]
pub(crate) enum RunFailure {
    StackOverflow,
    Trap,
    BadExport,
    RuntimeError,
}

pub(crate) fn status_name(status: i32) -> &'static str {
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

pub(crate) fn classify_run_error(error: &anyhow::Error) -> RunFailure {
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

pub(crate) fn classify_child_error(error: &wasmtime::Error) -> i32 {
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
