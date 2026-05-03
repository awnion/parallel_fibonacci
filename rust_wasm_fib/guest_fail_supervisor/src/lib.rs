#[runtime::fail_supervisor_component]
fn supervise_fib(n: u64, max_retries: u32) -> runtime::SupervisorReport {
    for attempt in 1..=max_retries {
        let (child_status, result) = runtime::spawn_link!(n);

        if child_status == runtime::STATUS_OK {
            return runtime::SupervisorReport {
                status: runtime::STATUS_OK,
                attempts: attempt,
                child_status,
                result,
            };
        }

        if attempt == max_retries {
            return runtime::SupervisorReport {
                status: runtime::STATUS_SUPERVISOR_FAILED,
                attempts: attempt,
                child_status,
                result: 0,
            };
        }
    }

    runtime::SupervisorReport {
        status: runtime::STATUS_SUPERVISOR_FAILED,
        attempts: 0,
        child_status: runtime::STATUS_OK,
        result: 0,
    }
}
