#[tokio::main]
async fn main() {
    const N: u64 = 52;
    println!("fib {N} = {}", rust_fib::fib(N).await);
}
