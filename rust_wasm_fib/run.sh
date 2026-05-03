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

target_dir="$(
  cargo metadata --format-version 1 --no-deps \
    | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])'
)"
wasm_path="$target_dir/wasm32-wasip2/release/rust_wasm_fib_guest.wasm"
host_path="$target_dir/release/rust_wasm_fib_host"

if [ "${RUST_WASM_FIB_BUILD:-0}" = "1" ] || [ ! -x "$host_path" ]; then
  cargo build -p rust_wasm_fib_host --release
fi

if [ "${RUST_WASM_FIB_BUILD:-0}" = "1" ] || [ ! -f "$wasm_path" ]; then
  cargo build -p rust_wasm_fib_guest --target wasm32-wasip2 --release
fi

exec "$host_path" "$n" "$threads" "$wasm_path"
