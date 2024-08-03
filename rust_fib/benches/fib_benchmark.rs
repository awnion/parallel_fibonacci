use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rust_fib::{fib, fib2, fib3, fib4};
use tokio::runtime::Builder;

fn criterion_benchmark(c: &mut Criterion) {
    let rt = Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Creating runtime failed");

    let size = black_box(40);

    c.bench_function(format!("fib {size}").as_str(), |b| {
        b.to_async(&rt).iter(|| fib(size))
    });

    c.bench_function(format!("fib2 {size}").as_str(), |b| {
        b.to_async(&rt).iter(|| fib2(size))
    });

    c.bench_function(format!("fib3 {size}").as_str(), |b| b.iter(|| fib3(size)));

    c.bench_function(format!("fib4 {size}").as_str(), |b| b.iter(|| fib4(size)));
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
