fn fib_cpu(n: u64) -> u64 {
    match n {
        0 => 0,
        1 => 1,
        n => fib_cpu(n - 1) + fib_cpu(n - 2),
    }
}

#[async_recursion::async_recursion]
async fn fib(n: u64) -> u64 {
    if n > 30 {
        let (a, b) = tokio::join!(
            tokio::task::spawn(async move { fib(n - 1).await }),
            fib(n - 2),
        );
        a.unwrap() + b
    } else {
        fib_cpu(n)
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 16)]
async fn main() {
    const N: u64 = 52;
    println!("fib {N} = {}", fib(N).await);
}
