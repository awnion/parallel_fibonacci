# Rust WASM Fibonacci

Generic native Rust host plus Rust WASM component implementation of the parallel
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
callable/export signature is `async fn(u64) -> u64`.

The host is not tied to this component. The scripts pass component paths,
export names, runtime import names, and arguments into the generic host commands:

```sh
rust_wasm_fib_host run-u64 <component> <function> <arg> [workers] [runtime-import]
rust_wasm_fib_host spawn-link-u64 <supervisor> <supervisor-fn> <child> <child-fn> \
  <arg> <retries> <child-init-fn|-> <child-init-arg> [runtime-import] [runtime-fn]
```

## Build

Build all prebuilt artifacts:

```sh
./build
```

This builds the native host once, the normal component guest, and the fail-demo
component guests.

It also writes WIT copies for external tooling, for example Python bindings, to
`target/generated-wit/rust_wasm_fib/`.

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

The fail demo uses two WASIp2 components. A WASM supervisor calls a
host-provided `runtime.spawn-link` component import. The host import creates a
fresh isolated child component instance, calls its exported `fib(n)`, catches
`Trap::StackOverflow`, drops that child instance, and returns an Erlang-style
linked process status to the supervisor.

```sh
./run_fail_demo.sh 52 3
```

The optional third argument controls CPU burn per recursive frame. The default
is calibrated on this machine to make each child stack overflow take roughly
3-4 seconds:

```sh
./run_fail_demo.sh 52 1 1700000
```
