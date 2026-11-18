---
title: WebAssembly Rust libraries link on macOS
date: 2026-11-16
---

A `rust_library` with `target = "wasm32-unknown-unknown"` and
`crate_type = "cdylib"` now links on macOS the way it does under Cargo.
Struo gives rustc the toolchain's library directory through
`DYLD_FALLBACK_LIBRARY_PATH`, as rustup does for Cargo, so the bundled
`rust-lld` finds `libLLVM.dylib`. The variable also survives the shell
that applies build script output.

WebAssembly dynamic libraries are now written as `<crate>.wasm` instead of
`lib<crate>.so`.

A first-party crate that sets `cargo_package` and depends on a
`cargo_dependencies` set now gets that package's Cargo environment, such as
`CARGO_PKG_NAME`, `CARGO_PKG_VERSION`, and `CARGO_MANIFEST_DIR`, so macros
such as `#[wasm_bindgen]` compile without setting them in `rustc_env`.
Values in `rustc_env` still win.
