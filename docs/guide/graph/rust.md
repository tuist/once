# Rust

Once builds Rust libraries and binaries from declarative `once.toml`
manifests. [`rust_library`](/reference/prelude/rust_library) emits an
rlib provider that downstream Rust targets consume through `--extern`.
[`rust_mobile_library`](/reference/prelude/rust_mobile_library) lets Apple
and Android consumers materialize native libraries from one Rust target.
[`rust_binary`](/reference/prelude/rust_binary) compiles an executable
from a main crate and its Rust deps, then exposes build and run capabilities.
[`rust_test`](/reference/prelude/rust_test)
compiles a Rust test crate with `rustc --test` and exposes Once's generic
test capability. [`rust_crate`](/reference/prelude/rust_crate) is the lowered
target shape for resolved third-party Cargo packages.
[`rust_proc_macro`](/reference/prelude/rust_proc_macro) compiles a procedural
macro for downstream Rust targets. [`cargo_dependencies`](/reference/prelude/cargo_dependencies)
groups resolved Cargo packages behind one cacheable graph target.

For the per-target-kind attribute, dep, provider, and capability tables see
the [target kind reference](/reference/prelude/).

## Targets

Declare first-party Rust libraries and binaries with the same target
shape as other graph target kinds:

```toml
[[target]]
name = "hello"
kind = "rust_library"
srcs = ["src/**/*.rs"]

[target.attrs]
crate_name = "hello"
edition = "2021"
```

Dependency references are root-relative by default. `./` and `../`
references resolve from the package that owns the manifest.
For host-specific dependencies, `deps` may use a `select` table with
branches such as `linux`, `macos-arm64`, `macos-x86_64`, `windows`,
and `default`:

```toml
[target.deps.select]
linux = ["third_party/rust/native-linux-1.0.0"]
"macos-arm64" = ["third_party/rust/native-macos-1.0.0"]
default = []
```

Rust targets accept Bazel and Buck2 style controls such as
`crate_root`, `features` or `crate_features`, `env` or `rustc_env`,
`rustc_env_files`, `rustc_flags`, `linker`, `linker_flags`, `linker_script`,
`crate_aliases`, `aliases`, `named_deps`, `cargo_package`, and `target` for
`rustc --target`. `compile_data` participates in compiler action keys, while
`data` is propagated to binary and test execution. Final artifact libraries can set
`crate_type` on `rust_library` to produce `staticlib`, `cdylib`, or
`dylib` outputs. Use `rust_mobile_library` when one target should expose
consumer-owned Apple static library and Android shared library variants.
`features` and `crate_features` may also use `select` with the same
host or target tokens.
Targets may also set `build_script` to compile and run a Cargo-style
build script before `rustc`; Once sets `OUT_DIR` for generated files.
Once consumes common build-script stdout directives:
`cargo:rustc-cfg`, `cargo:rustc-check-cfg`, `cargo:rustc-env`,
`cargo:rustc-link-arg`, `cargo:rustc-link-lib`, and
`cargo:rustc-link-search`. Dependency `cargo:rustc-link-search`
outputs are also replayed for downstream Rust targets, so native
libraries referenced by dependency metadata can be found at final link
time. For crates with Cargo `links` metadata, custom metadata from
direct dependency build scripts is exposed to downstream build scripts
as `DEP_<LINKS>_<KEY>` environment variables.
Generated Cargo dependencies set `cap_lints = "allow"` so dependency
crates follow Cargo's lint-capping behavior.

Rust binaries accept `args`, `run_env`, and `env_inherit` for `once run`.
Runtime data from their Rust dependency graph is declared as run action inputs,
so fixtures stay visible to execution and scheduling without becoming compiler
action inputs.

Declare Rust tests as separate `rust_test` targets. Unit tests can use the
library root as their `crate_root`; integration tests usually point
`crate_root` at a file under `tests/` and depend on the library under test:

```toml
[[target]]
name = "hello_tests"
kind = "rust_test"
srcs = ["tests/**/*.rs"]
deps = ["./hello"]

[target.attrs]
crate_name = "hello_tests"
crate_root = "tests/greeting_test.rs"
labels = ["unit"]
```

## Native Mobile Outputs

Rust shared code can feed Apple and Android targets through native provider
fields. Use `rust_mobile_library` when the same Rust sources should feed
both Apple consumers such as `apple_application` and Android consumers such
as `android_binary`. Each consumer declares only the variant it needs: Apple
consumers materialize a `staticlib`, and Android consumers materialize a
`cdylib`. The platform compiles use separate target triples, outputs, scratch
files, and action identifiers.

Android dynamic libraries also need an Android
[Application Binary Interface](https://developer.android.com/ndk/guides/abis)
directory, which Once can infer from common Android target triples or read
from `android_abi`. When `ANDROID_NDK_HOME` or `android_ndk` is available,
Once uses the Android Native Development Kit clang wrapper as the default
linker for Android targets. Set `android_api` to choose the Android platform
level.

`rust_mobile_library` does not support Rust dependencies yet. Use explicit
platform-specific `rust_library` targets when the shared Rust code depends on
other Rust crates.

```toml
[[target]]
name = "SharedRust"
kind = "rust_mobile_library"
srcs = ["src/**/*.rs"]

[target.attrs]
crate_name = "shared_rust"
apple_target = "aarch64-apple-ios"
android_target = "aarch64-linux-android"
android_abi = "arm64-v8a"
android_api = 24
```

## Dependency Resolution

Keep external Rust dependencies in `Cargo.toml` and `Cargo.lock`.
Declare one cacheable `cargo_dependencies` target that reads those
Cargo files:

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

[[target]]
name = "hello"
kind = "rust_binary"
deps = ["cargo_dependencies"]
srcs = ["src/**/*.rs"]

[target.attrs]
crate_name = "hello"

[target.attrs.rustc_env]
CARGO_MANIFEST_DIR = "apps/hello"
CARGO_PKG_NAME = "hello"
CARGO_PKG_VERSION = "0.0.0"
```

`cargo_dependencies` invokes `cargo metadata --locked`, optionally with
`--filter-platform`, `--features`, `--all-features`, or
`--no-default-features` based on its attributes. Registry and git
packages become cacheable Once `rust_crate` or `rust_proc_macro`
actions compiled from the configured `vendor_dir`; workspace and path
packages stay first-party Once targets.

The provider includes Cargo's direct external dependency set for each
workspace package. Rust targets use `CARGO_PKG_NAME` from `rustc_env`,
or `cargo_package` when set, to select the external deps Cargo
resolved for that package. This keeps the lockfile and Cargo manifests
authoritative while still reconciling resolved crates into Once's
caching model.

The resolver preserves custom library paths, active features,
proc-macro crate types, package env, renamed deps, build-deps for
packages with build scripts, dependency lint capping, Cargo `links`
metadata, and build scripts.

The `target` attribute is a per-target `rustc --target` setting. A
complete cross-compiled binary graph still needs target-specific deps
and host-built proc macro deps represented explicitly in the graph.
Once does not yet synthesize that multi-configuration split from a
single target.

## Commands

Inspect the graph with [`once query`](/reference/cli/query):

```sh
once query targets --kind rust_library
once query schema rust_binary
once query schema cargo_dependencies
```

Build Rust graph targets with [`once build`](/reference/cli/build):

```sh
once build crates/hello/hello
```

Outputs land under `.once/out/<target>/`. The target kind reference pages list
the exact outputs each target kind emits.

## Prior Art

The Rust target kind set follows the same provider model used by established
Rust build target kinds:

- Bazel Rust target kinds model Rust libraries as crate providers, pass direct
  dependencies through `--extern`, and make transitive artifacts
  available for downstream compilation.
- Buck2 Rust target kinds separate libraries, binaries, crate metadata,
  dependency contexts, and Cargo package lowering so third-party crates
  become normal graph nodes.

Once is not Buck-compatible, Bazel-compatible, or a drop-in
replacement for Cargo. The goal is an inspectable graph where agents
can discover target kind contracts, materialize examples, and edit targets
through the same Once surfaces.
