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
- [`android_local_test`](/reference/prelude/android_local_test): Android
  Java and Kotlin local tests run on the host Java virtual machine
- [`android_instrumentation_test`](/reference/prelude/android_instrumentation_test):
  Android instrumentation tests run on a device or emulator
- [`android_binary`](/reference/prelude/android_binary): Android APK
  built from Java sources, resources, native libraries, Android resource
  deps, and Android library deps

## Swift target kinds

- [`swift_android_library`](/reference/prelude/swift_android_library):
  Swift shared library compiled for Android and packaged through APK
  native-library providers

## Kotlin target kinds

- [`kotlin_apple_framework`](/reference/prelude/kotlin_apple_framework):
  Kotlin/Native framework bundle consumed by Apple application and test
  targets

## Elixir target kinds

- [`elixir_library`](/reference/prelude/elixir_library): Elixir
  application compiled into cacheable bytecode with a target-level compiler
  action
- [`elixir_test`](/reference/prelude/elixir_test): ExUnit tests run against
  an already compiled Elixir application

## Rust target kinds

- [`cargo_dependencies`](/reference/prelude/cargo_dependencies): cacheable
  Cargo dependency set consumed by Rust targets
- [`rust_library`](/reference/prelude/rust_library): Rust rlib consumed
  by downstream Rust targets, or native static/shared libraries consumed
  by Apple and Android targets
- [`rust_mobile_library`](/reference/prelude/rust_mobile_library): Rust
  native library materialized by Apple and Android consumers under one
  target label
- [`rust_binary`](/reference/prelude/rust_binary): Rust executable built
  from a main crate and Rust deps
- [`rust_test`](/reference/prelude/rust_test): Rust test crate compiled
  with `rustc --test` and run through Once's test capability
- [`rust_crate`](/reference/prelude/rust_crate): resolved third-party
  Cargo package lowered into a Rust library target
- [`rust_proc_macro`](/reference/prelude/rust_proc_macro): Rust
  procedural macro compiled for Rust targets
