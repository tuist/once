# Rust

Fabrik supports granular Rust targets in `fabrik.toml`. Each granular target becomes one cacheable action.

```toml
[[rust.library]]
name = "greeting"
srcs = ["src/lib.rs"]

[[rust.binary]]
name = "hello"
srcs = ["src/main.rs"]
deps = ["//examples/rust-app:greeting"]

[[rust.test]]
name = "greeting_test"
srcs = ["tests/greeting.rs"]
crate_root = "tests/greeting.rs"
deps = ["//examples/rust-app:greeting"]
```

Run it:

```sh
fabrik build //examples/rust-app:hello
fabrik test //examples/rust-app:greeting_test
```

## Cargo Escape Hatch

Use `cargo.binary` when the Cargo graph needs features that the granular path does not support yet, such as third-party build-script wiring.

```toml
[[cargo.binary]]
name = "fabrik"
cargo_package = "fabrik-cli"
bin = "fabrik"
srcs = ["Cargo.lock", "Cargo.toml", "crates/fabrik-cli/src/main.rs"]
```

Run it:

```sh
fabrik run //:fabrik
```

## Cache Behavior

- `rust.library`, `rust.binary`, `rust.test`, and `rust.proc_macro` build actions are cacheable.
- `fabrik test` also caches the test binary execution as a separate action.
- Declared outputs are restored from the CAS on cache hits.
- A source edit invalidates the changed crate and reverse dependencies, not unrelated crates.
