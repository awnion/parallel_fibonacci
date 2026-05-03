#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    let n: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(52);

    let mode = args.get(2).map(|s| s.as_str()).unwrap_or("tokio");

    let result = match mode {
        "threads" => rust_fib::fib3(n),
        _ => rust_fib::fib(n).await,
    };

    println!("fib {n} = {result}");
}
