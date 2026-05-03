# Rust WASM Fibonacci

Native Rust host plus Rust WASM guest implementation of the parallel Fibonacci
example.

The host reads CLI parameters, embeds Wasmtime, loads the guest module, calls
`init(worker_threads)`, then calls the exported WASM function `fib(n)` and prints
the result.

The guest is compiled to `wasm32-wasip1-threads`. Above `n = 30`, `fib` spawns
the left recursive branch as a lightweight async task, computes the right branch,
then awaits the task handle. The guest runs these tasks on an `async-executor`
instance backed by a fixed pool of WASI threads provided by the host's Wasmtime
linker.

This intentionally uses WASIp1 for the guest imports because Rust's threaded
WASI target is `wasm32-wasip1-threads` and Wasmtime's `wasmtime-wasi-threads`
crate implements the wasi-threads `thread-spawn` import for that model. The host
uses the current Wasmtime embedding API, but switching the guest to plain
`wasm32-wasip2` would remove the thread-spawn path needed for this benchmark.

## Requirements

```sh
rustup target add wasm32-wasip1-threads
```

## Run

Recommended command:

```sh
./run.sh 52
```

`run.sh` builds the WASM guest, detects the host logical CPU count, and starts
the native host binary.

To set the guest worker count explicitly:

```sh
./run.sh 52 8
```

You can also set `RUST_WASM_FIB_THREADS`; the CLI argument wins when both are
provided.

```sh
RUST_WASM_FIB_THREADS=8 ./run.sh 52
```

## Manual Build

```sh
cargo build -p rust_wasm_fib_guest --target wasm32-wasip1-threads --release
cargo run -p rust_wasm_fib --release -- 52 8 \
  target/wasm32-wasip1-threads/release/rust_wasm_fib_guest.wasm
```

If Cargo is configured with a custom target directory, pass the actual guest
`.wasm` path as the third argument or set `RUST_WASM_FIB_GUEST`.

## Stack Overflow Supervisor Demo

The fail demo runs a WASM supervisor that calls a host-provided
`runtime.run-fib` import. The host import creates a fresh isolated child WASM
instance, calls its exported `fib(n)`, catches `Trap::StackOverflow`, drops that
child instance, and returns a status code to the supervisor.

The supervisor retries three times by default and then reports failure itself:

```sh
./run_fail_demo.sh 52 3
```

The optional third argument controls CPU burn per recursive frame. The default
is calibrated on this machine to make each child stack overflow take roughly
3-4 seconds:

```sh
./run_fail_demo.sh 52 1 1700000
```

Expected shape:

```text
runtime.run-fib: child status=1 (stack-overflow)
runtime.run-fib: child status=1 (stack-overflow)
runtime.run-fib: child status=1 (stack-overflow)
Supervisor status = 10 (supervisor-failed)
Supervisor attempts = 3
Child burn iterations/frame = 1700000
Last child status = 1 (stack-overflow)
Runtime survived child traps; supervisor reported failure after retries.
```
