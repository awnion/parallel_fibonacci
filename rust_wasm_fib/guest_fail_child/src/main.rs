use std::sync::atomic::{AtomicU64, Ordering};

const DEFAULT_BURN_ITERS: u64 = 1_700_000;

static BURN_ITERS: AtomicU64 = AtomicU64::new(DEFAULT_BURN_ITERS);
static STOP_AT: AtomicU64 = AtomicU64::new(u64::MAX);

#[inline(never)]
fn burn(depth: u64) -> u64 {
    let mut value = depth ^ 0x9e37_79b9_7f4a_7c15;

    for _ in 0..BURN_ITERS.load(Ordering::Relaxed) {
        value = value
            .rotate_left(7)
            .wrapping_mul(0xbf58_476d_1ce4_e5b9)
            .wrapping_add(0x94d0_49bb_1331_11eb);
        value = std::hint::black_box(value);
    }

    value
}

#[inline(never)]
fn overflow(depth: u64) -> u64 {
    let value = burn(depth);

    if STOP_AT.load(Ordering::Relaxed) == std::hint::black_box(value) {
        return depth;
    }

    overflow(depth.wrapping_add(1)).wrapping_add(1)
}

#[unsafe(no_mangle)]
pub extern "C" fn init(burn_iters: u64) {
    BURN_ITERS.store(burn_iters, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn fib(n: u64) -> u64 {
    overflow(n)
}

fn main() {}
