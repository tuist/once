# Rust App

This example represents a small Rust project declared directly in
`fabrik.toml`.

It contains:

- a `rust.library` target named `greeting`
- a `rust.binary` target named `hello`
- a `rust.test` target named `greeting_test`

Run it from the repository root:

```sh
mise exec -- target/release/fabrik build //examples/rust/granular/basic-app:hello
mise exec -- target/release/fabrik test //examples/rust/granular/basic-app:greeting_test
```
