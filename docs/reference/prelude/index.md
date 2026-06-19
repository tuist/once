# Prelude

The Starlark prelude bundled with `once` declares every built-in target
kind: its attribute schema, dep edges, providers, capabilities, and
the impl that turns a target into a set of cached actions. The
[`once query schema`](/reference/cli/query) command exposes the same
metadata at the keyboard.

## Apple target kinds

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

## Android target kinds

- [`android_resource`](/reference/prelude/android_resource): Android
  resources and assets compiled into a static resource package
- [`android_library`](/reference/prelude/android_library): Android
  Java library packaged as a jar, static resources, and AAR
- [`android_binary`](/reference/prelude/android_binary): Android APK
  built from Java sources, resources, Android resource deps, and Android
  library deps

## Rust target kinds

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
