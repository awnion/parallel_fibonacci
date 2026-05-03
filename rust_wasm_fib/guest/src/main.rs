use async_executor::Executor;
use futures_lite::future;
use std::num::NonZeroUsize;
use std::sync::{Once, OnceLock};

const PARALLEL_CUTOFF: u64 = 30;

static EXECUTOR: OnceLock<&'static Executor<'static>> = OnceLock::new();
static START_WORKERS: Once = Once::new();

fn fib_cpu(n: u64) -> u64 {
    match n {
        0 => 0,
        1 => 1,
        n => fib_cpu(n - 1) + fib_cpu(n - 2),
    }
}

#[async_recursion::async_recursion]
async fn fib_async(n: u64) -> u64 {
    if n > PARALLEL_CUTOFF {
        let left = executor().spawn(fib_async(n - 1));
        let right = fib_async(n - 2).await;
        left.await + right
    } else {
        fib_cpu(n)
    }
}

fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(1)
}

fn executor() -> &'static Executor<'static> {
    EXECUTOR
        .get()
        .copied()
        .expect("guest async executor must be initialized before use")
}

fn init_executor(worker_threads: usize) {
    let executor = *EXECUTOR.get_or_init(|| Box::leak(Box::new(Executor::new())));
    let worker_threads = worker_threads.max(1);

    START_WORKERS.call_once(|| {
        for index in 1..worker_threads {
            std::thread::Builder::new()
                .name(format!("guest-async-worker-{index}"))
                .spawn(move || {
                    future::block_on(executor.run(future::pending::<()>()));
                })
                .expect("failed to spawn guest async worker");
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn init(worker_threads: u32) {
    init_executor(worker_threads as usize);
}

#[unsafe(no_mangle)]
pub extern "C" fn fib(n: u64) -> u64 {
    init_executor(default_threads());
    let root = executor().spawn(fib_async(n));
    future::block_on(executor().run(root))
}

fn main() {}

#[cfg(test)]
mod tests {
    use super::fib;

    #[test]
    fn computes_known_small_values() {
        let values = [0, 1, 1, 2, 3, 5, 8, 13, 21, 34, 55];

        for (n, expected) in values.into_iter().enumerate() {
            assert_eq!(fib(n as u64), expected);
        }
    }

    #[test]
    fn computes_parallel_branch_value() {
        assert_eq!(fib(35), 9_227_465);
    }
}
