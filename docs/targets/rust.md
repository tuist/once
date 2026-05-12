# Rust

Fabrik supports granular Rust targets in `fabrik.toml`. Each granular target becomes one cacheable action.

```toml
[[rust.library]]
name = "greeting"
srcs = ["src/lib.rs"]

[[rust.binary]]
name = "hello"
srcs = ["src/main.rs"]
deps = ["greeting"]

[[rust.test]]
name = "greeting_test"
srcs = ["tests/greeting.rs"]
crate_root = "tests/greeting.rs"
deps = ["greeting"]
```

Run it:

```sh
fabrik build examples/rust/granular/basic-app/hello
fabrik test examples/rust/granular/basic-app/greeting_test
```

## Build Scripts

Use `cargo.build_script` when a granular Rust target needs `build.rs`
directives such as `cargo::rustc-cfg`, `cargo::rustc-env`,
`cargo::rustc-link-lib`, or `cargo::rustc-link-search`.

```toml
[[cargo.build_script]]
name = "build"
srcs = ["build.rs", "config.txt"]

[[rust.library]]
name = "native"
srcs = ["src/lib.rs"]
deps = ["build"]
```

The build script is a normal cacheable node. Its captured directives
are restored from the CAS on cache hits before dependent `rustc`
actions run.

## Cargo Escape Hatch

Use `cargo.binary` when the Cargo graph needs features that the granular path does not support yet, such as Cargo workspace import or per-target feature resolution.

```toml
[[cargo.binary]]
name = "fabrik"
cargo_package = "fabrik-cli"
bin = "fabrik"
srcs = ["Cargo.lock", "Cargo.toml", "crates/fabrik-cli/src/main.rs"]
```

Run it:

```sh
fabrik run fabrik
```

## Cache Behavior

- `rust.library`, `rust.binary`, `rust.test`, and `rust.proc_macro` build actions are cacheable.
- `fabrik test` also caches the test binary execution as a separate action.
- Declared outputs are restored from the CAS on cache hits.
- A source edit invalidates the changed crate and reverse dependencies, not unrelated crates.
