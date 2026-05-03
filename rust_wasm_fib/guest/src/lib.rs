use async_recursion::async_recursion;

fn fib_cpu(n: u64) -> u64 {
    match n {
        0 => 0,
        1 => 1,
        n => fib_cpu(n - 1) + fib_cpu(n - 2),
    }
}

#[runtime::component]
#[async_recursion(?Send)]
async fn fib(n: u64) -> u64 {
    if n > 30 {
        let left = runtime::spawn!(fib(n - 1));
        let right = fib(n - 2).await;

        match left.join().await {
            Ok(left) => left + right,
            Err(_) => 0,
        }
    } else {
        fib_cpu(n)
    }
}

#[cfg(test)]
mod tests {
    use super::fib_cpu;

    #[test]
    fn computes_known_small_values() {
        let values = [0, 1, 1, 2, 3, 5, 8, 13, 21, 34, 55];

        for (n, expected) in values.into_iter().enumerate() {
            assert_eq!(fib_cpu(n as u64), expected);
        }
    }
}
