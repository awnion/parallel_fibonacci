use rust_fib::fib2;

#[tokio::main]
async fn main() {
    const N: u64 = 52;
    println!("fib {N} = {}", fib2(N).await);
}
