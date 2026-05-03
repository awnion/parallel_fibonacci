#!/usr/bin/env sh
set -eu

n="${1:-52}"
retries="${2:-3}"
burn_iters="${3:-${RUST_WASM_FIB_FAIL_BURN_ITERS:-1700000}}"

target_dir="$(
  cargo metadata --format-version 1 --no-deps \
    | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])'
)"

supervisor_path="$target_dir/wasm32-wasip2/release/rust_wasm_fib_guest_fail_supervisor.wasm"
child_path="$target_dir/wasm32-wasip2/release/rust_wasm_fib_guest_fail_child.wasm"
host_path="$target_dir/release/rust_wasm_fib_host"

if [ "${RUST_WASM_FIB_BUILD:-0}" = "1" ] || [ ! -x "$host_path" ]; then
  cargo build -p rust_wasm_fib_host --release
fi

if [ "${RUST_WASM_FIB_BUILD:-0}" = "1" ] || [ ! -f "$child_path" ]; then
  cargo build -p rust_wasm_fib_guest_fail_child --target wasm32-wasip2 --release
fi

if [ "${RUST_WASM_FIB_BUILD:-0}" = "1" ] || [ ! -f "$supervisor_path" ]; then
  cargo build -p rust_wasm_fib_guest_fail_supervisor --target wasm32-wasip2 --release
fi

exec "$host_path" supervise-u64 \
  "$supervisor_path" supervise-fib \
  "$child_path" fib \
  "$n" "$retries" init "$burn_iters" \
  rust-wasm-runtime:supervisor/runtime run-child
