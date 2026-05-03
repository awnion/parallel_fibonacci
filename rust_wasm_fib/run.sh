#!/usr/bin/env sh
set -eu

detect_threads() {
  if [ -n "${RUST_WASM_FIB_THREADS:-}" ]; then
    printf '%s\n' "$RUST_WASM_FIB_THREADS"
    return
  fi

  if command -v getconf >/dev/null 2>&1; then
    threads="$(getconf _NPROCESSORS_ONLN 2>/dev/null || true)"
    if [ -n "$threads" ] && [ "$threads" -gt 0 ] 2>/dev/null; then
      printf '%s\n' "$threads"
      return
    fi
  fi

  if command -v sysctl >/dev/null 2>&1; then
    threads="$(sysctl -n hw.logicalcpu 2>/dev/null || true)"
    if [ -n "$threads" ] && [ "$threads" -gt 0 ] 2>/dev/null; then
      printf '%s\n' "$threads"
      return
    fi
  fi

  printf '1\n'
}

n="${1:-52}"
threads="${2:-$(detect_threads)}"

cargo build -p rust_wasm_fib_guest --target wasm32-wasip1-threads --release

target_dir="$(
  cargo metadata --format-version 1 --no-deps \
    | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])'
)"
wasm_path="$target_dir/wasm32-wasip1-threads/release/rust_wasm_fib_guest.wasm"

exec cargo run -p rust_wasm_fib --release -- "$n" "$threads" "$wasm_path"
