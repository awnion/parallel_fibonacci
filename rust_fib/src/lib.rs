fn fib_cpu(n: u64) -> u64 {
    match n {
        0 => 0,
        1 => 1,
        n => fib_cpu(n - 1) + fib_cpu(n - 2),
    }
}

#[async_recursion::async_recursion]
pub async fn fib(n: u64) -> u64 {
    if n > 30 {
        let first = tokio::spawn(async move { fib(n - 1).await });
        fib(n - 2).await + first.await.unwrap()
    } else {
        fib_cpu(n)
    }
}

pub fn fib2(n: u64) -> impl std::future::Future<Output = u64> + Send {
    async move {
        if n > 30 {
            let first = tokio::spawn(async move { fib2(n - 1).await });
            Box::pin(fib2(n - 2)).await + first.await.unwrap()
        } else {
            fib_cpu(n)
        }
    }
}
