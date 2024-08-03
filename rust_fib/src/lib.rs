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

pub fn fib3(n: u64) -> u64 {
    use std::sync::mpsc::channel;
    if n > 30 {
        let (sender, receiver) = channel();
        std::thread::spawn(move || {
            sender.send(fib3(n - 1)).unwrap();
        });
        let b = fib3(n - 2);
        receiver.recv().unwrap() + b
    } else {
        fib_cpu(n)
    }
}

pub fn fib4(n: u64) -> u64 {
    use kanal::unbounded as channel;
    if n > 30 {
        let (sender, receiver) = channel();
        std::thread::spawn(move || {
            sender.send(fib4(n - 1)).unwrap();
        });
        let b = fib4(n - 2);
        receiver.recv().unwrap() + b
    } else {
        fib_cpu(n)
    }
}
