# Prelude

The Starlark prelude bundled with `once` declares every built-in rule
kind: its attribute schema, dep edges, providers, capabilities, and
the impl that turns a target into a set of cached actions. The
[`once query schema`](/reference/cli/query) command exposes the same
metadata at the keyboard.

## Apple rules

- [`apple_library`](/reference/prelude/apple_library): Swift,
  Objective-C, C, and C++ static library
- [`swift_macro`](/reference/prelude/swift_macro): Swift
  compiler-plugin dylib loaded by `apple_library` deps
- [`apple_framework`](/reference/prelude/apple_framework): dynamic
  Apple framework bundle
- [`apple_application`](/reference/prelude/apple_application): Apple
  application bundle
- [`apple_test_bundle`](/reference/prelude/apple_test_bundle): XCTest
  bundle assembled for an external runner

## Rust rules

- [`cargo_dependencies`](/reference/prelude/cargo_dependencies): cacheable
  Cargo dependency set consumed by Rust targets
- [`rust_library`](/reference/prelude/rust_library): Rust rlib consumed
  by downstream Rust targets
- [`rust_binary`](/reference/prelude/rust_binary): Rust executable built
  from a main crate and Rust deps
- [`rust_crate`](/reference/prelude/rust_crate): resolved third-party
  Cargo package lowered into a Rust library target
- [`rust_proc_macro`](/reference/prelude/rust_proc_macro): Rust
  procedural macro compiled for Rust targets
