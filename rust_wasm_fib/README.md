# Rust WASM Fibonacci

Native Rust host plus Rust WASM component implementation of the parallel
Fibonacci example.

The normal guest is a WASI Preview 2 component with Preview 3 async component
ABI enabled in Wasmtime. The guest imports a small runtime API:

```wit
spawn: func(function: string, payload: list<u8>) -> task
await-task: async func(task: task) -> result<list<u8>, child-status>
```

Guest business code uses the separate `runtime` crate instead of raw runtime
imports, handwritten WIT bindings, or handwritten component exports:

```rust
#[runtime::component]
async fn fib(n: u64) -> u64 {
    // ...
}

let left = runtime::spawn!(fib(n - 1));
let right = fib(n - 2).await;
let left = left.join().await?;
```

`#[runtime::component]` defaults to `world = "fib-guest"` and generates inline
WIT for the exported function. It also generates `wit_bindgen`, the `Component`
marker, `impl Guest`, `export!(Component)`, and the hidden typed call
descriptor/codec.
`runtime::spawn!` expands to that descriptor, so a missing callable or
unsupported signature fails at compile time. For this POC the supported
callable/export signature is `async fn(u64) -> u64`, and the host dynamically
looks up the exported component function by name before calling it in a fresh
Store/Instance.

## Build

Build all prebuilt artifacts:

```sh
./build
```

This builds the native host once, the normal component guest, and the WASIp1
fail-demo guests.

It also writes a WIT copy for external tooling, for example Python bindings, to
`target/generated-wit/rust_wasm_fib/world.wit`.

## Run

Fast path using already-built artifacts:

```sh
./run.sh 52
./run.sh 52 8
```

Force rebuild before running:

```sh
RUST_WASM_FIB_BUILD=1 ./run.sh 52 8
```

`run.sh` detects logical CPU count when the worker argument is omitted. You can
also set `RUST_WASM_FIB_THREADS`.

## Stack Overflow Supervisor Demo

The fail demo still uses core WASIp1 modules. A WASM supervisor calls a
host-provided `runtime.run-fib` import. The host import creates a fresh isolated
child WASM instance, calls its exported `fib(n)`, catches
`Trap::StackOverflow`, drops that child instance, and returns a status code to
the supervisor.

```sh
./run_fail_demo.sh 52 3
```

The optional third argument controls CPU burn per recursive frame. The default
is calibrated on this machine to make each child stack overflow take roughly
3-4 seconds:

```sh
./run_fail_demo.sh 52 1 1700000
```
