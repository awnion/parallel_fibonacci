#[tokio::main]
async fn main() {
    const N: u64 = 52;
    // println!("fib {N} = {}", fib2(N).await);
    println!("fib {N} = {}", rust_fib::fib4(N));
}
