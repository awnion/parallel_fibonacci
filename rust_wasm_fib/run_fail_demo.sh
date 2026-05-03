#!/usr/bin/env sh
set -eu

n="${1:-52}"
retries="${2:-3}"
burn_iters="${3:-${RUST_WASM_FIB_FAIL_BURN_ITERS:-1700000}}"

cargo build -p rust_wasm_fib_guest_fail_child --target wasm32-wasip1 --release
cargo build -p rust_wasm_fib_guest_fail_supervisor --target wasm32-wasip1 --release

target_dir="$(
  cargo metadata --format-version 1 --no-deps \
    | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])'
)"

supervisor_path="$target_dir/wasm32-wasip1/release/rust_wasm_fib_guest_fail_supervisor.wasm"
child_path="$target_dir/wasm32-wasip1/release/rust_wasm_fib_guest_fail_child.wasm"

exec cargo run -p rust_wasm_fib --release -- fail-demo \
  "$n" "$retries" "$supervisor_path" "$child_path" "$burn_iters"
