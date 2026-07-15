---
prev: false
next: false
---

# Rust

Once can build Rust libraries, binaries, tests, procedural macros, and native
mobile libraries. This guide starts with one local library, a binary that uses
it, and a test for the same code. Cargo dependency resolution comes after that
first working graph.

## Prerequisites

Install the repository's pinned Rust toolchain through mise:

```sh
mise install
mise exec -- rustc --version
```

Host binaries and tests also need the platform linker selected by the Rust
compiler. Cross-compiled and mobile outputs require the linker and target
support for their destination platform.

## Declare a Library, Binary, and Test

Create `apps/hello/once.toml`:

```toml
[[target]]
name = "greeting"
kind = "rust_library"
srcs = ["src/lib.rs"]

[target.attrs]
crate_name = "greeting"
edition = "2021"

[[target]]
name = "hello"
kind = "rust_binary"
srcs = ["src/main.rs"]
deps = ["./greeting"]

[target.attrs]
crate_name = "hello"
edition = "2021"

[[target]]
name = "greeting_tests"
kind = "rust_test"
srcs = ["tests/greeting_test.rs"]
deps = ["./greeting"]

[target.attrs]
crate_name = "greeting_tests"
crate_root = "tests/greeting_test.rs"
edition = "2021"
labels = ["unit"]
```

Use this source layout:

```text
apps/hello/
├── once.toml
├── src/
│   ├── lib.rs
│   └── main.rs
└── tests/
    └── greeting_test.rs
```

The library crate name is `greeting`, so the binary and test can refer to it
as `greeting` in Rust source. Their `./greeting` dependency gives the compiler
the matching built crate.

## Query Before Building

Inspect the three targets and their capabilities:

```sh
once query targets --kind rust_library
once query capabilities apps/hello/greeting
once query capabilities apps/hello/hello
once query capabilities apps/hello/greeting_tests
once query schema rust_binary
```

The library exposes `build`, the binary exposes `build` and `run`, and the test
target exposes `build` and `test`.

## Build, Run, and Test

Build the binary. Once builds `greeting` first because the binary depends on
it:

```sh
once build apps/hello/hello
```

Run that same binary:

```sh
once run apps/hello/hello
```

Run the test target:

```sh
once test apps/hello/greeting_tests
```

Outputs are materialized under `.once/out/<target>/`. The
[`rust_binary` reference](/reference/prelude/rust_binary) and
[`rust_test` reference](/reference/prelude/rust_test) list their executable,
log, and test-result outputs.

`rust_binary` accepts `args`, `run_env`, and `env_inherit` for runtime
configuration. `data` files become declared run inputs, while `compile_data`
files affect compilation. Keeping those roles separate makes cache behavior
visible.

## Add Cargo Dependencies

Keep third-party requirements in `Cargo.toml` and exact versions in
`Cargo.lock`. A root `cargo_dependencies` target lets Cargo resolve the
packages while Once builds the resolved crates as graph dependencies:

```toml
[[target]]
name = "cargo_dependencies"
kind = "cargo_dependencies"
srcs = [
  "Cargo.toml",
  "Cargo.lock",
  "apps/*/Cargo.toml",
]

[target.attrs]
manifest = "Cargo.toml"
lockfile = "Cargo.lock"
vendor_dir = "third_party/rust/vendor"
```

Add that target to a first-party Rust target and identify the matching Cargo
package:

```toml
[[target]]
name = "hello"
kind = "rust_binary"
srcs = ["src/main.rs"]
deps = ["./greeting", "cargo_dependencies"]

[target.attrs]
crate_name = "hello"
edition = "2021"

[target.attrs.rustc_env]
CARGO_MANIFEST_DIR = "apps/hello"
CARGO_PKG_NAME = "hello"
CARGO_PKG_VERSION = "0.0.0"
```

The dependency target runs `cargo metadata --locked`. Registry and Git
packages come from the configured vendor directory. Workspace and path
packages remain first-party Once targets. The Cargo manifests and lockfile
stay authoritative for package names, versions, active features, renamed
dependencies, procedural macros, and build dependencies.

Use [`once query schema cargo_dependencies`](/reference/prelude/cargo_dependencies)
before adding feature or target filters. A complete cross-compiled binary can
need destination-specific dependencies and host-built procedural macros.
Once does not synthesize that split from one target, so represent those
configurations explicitly.

## Use Build Scripts and Advanced Compiler Inputs

Rust targets can set `build_script` to compile and run a Cargo-style build
script before the main crate. Once provides `OUT_DIR` and consumes common
compiler configuration, environment, link argument, link library, and link
search directives printed by the script. Dependency link-search outputs and
Cargo `links` metadata are available to downstream targets and build scripts.

The target kind reference also documents compiler flags, environment files,
linker settings, crate aliases, feature selection, and host-specific
dependency selection. Add these only when the simple library edge above is
not enough, and query the schema before choosing an attribute.

## Produce Native Mobile Libraries

Use [`rust_mobile_library`](/reference/prelude/rust_mobile_library) when the
same sources feed both Apple and Android consumers:

```toml
[[target]]
name = "SharedRust"
kind = "rust_mobile_library"
srcs = ["src/shared/**/*.rs"]

[target.attrs]
crate_name = "shared_rust"
apple_target = "aarch64-apple-ios"
android_target = "aarch64-linux-android"
android_abi = "arm64-v8a"
android_api = 24
```

An Apple consumer requests a static library. An Android consumer requests a
shared library and packages it for the configured
[Application Binary Interface](https://developer.android.com/ndk/guides/abis).
Android linking requires the
[Android Native Development Kit](https://developer.android.com/ndk), found
through `ANDROID_NDK_HOME` or `android_ndk`.

`rust_mobile_library` does not support Rust dependencies yet. Use explicit
platform-specific `rust_library` targets when the shared code depends on other
Rust crates.

## Supported Target Kinds and Limitations

Use the target kind reference for each role:

- [`rust_library`](/reference/prelude/rust_library)
- [`rust_binary`](/reference/prelude/rust_binary)
- [`rust_test`](/reference/prelude/rust_test)
- [`rust_proc_macro`](/reference/prelude/rust_proc_macro)
- [`cargo_dependencies`](/reference/prelude/cargo_dependencies)
- [`rust_crate`](/reference/prelude/rust_crate)
- [`rust_mobile_library`](/reference/prelude/rust_mobile_library)

Rust tests run only host-target executables. A cross-target test can be built,
but running it requires a platform runner that this target kind does not
provide. Compatibility attributes listed as unsupported in the target kind
reference fail validation when set to a non-empty value.

## Next

Continue with [Memory](/guide/memory/) once the binary builds and tests. It
shows how Once records durable context about graph work. For Apple or Android
consumers of the Rust library, follow the relevant application guide first,
then add the native dependency after the application works independently.
