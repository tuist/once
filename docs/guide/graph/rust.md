# Rust Graph

Once builds Rust libraries and binaries from declarative `once.toml`
manifests. [`rust_library`](/reference/prelude/rust_library) emits an
rlib provider that downstream Rust targets consume through `--extern`.
[`rust_binary`](/reference/prelude/rust_binary) compiles an executable
from a main crate and its Rust deps. [`rust_crate`](/reference/prelude/rust_crate)
is the lowered target shape for resolved third-party Cargo packages.
[`rust_proc_macro`](/reference/prelude/rust_proc_macro) compiles a
procedural macro for downstream Rust targets. [`cargo_dependencies`](/reference/prelude/cargo_dependencies)
groups resolved Cargo packages behind one cacheable graph target.

For the per-rule attribute, dep, provider, and capability tables see
the [Prelude reference](/reference/prelude/).

## Targets

Declare first-party Rust libraries and binaries with the same target
shape as other graph rules:

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
`rustc_flags`, `linker`, `linker_flags`, `crate_aliases`, `cargo_package`,
and `target` for `rustc --target`. Final artifact libraries can set
`crate_type` on `rust_library` to produce `staticlib`, `cdylib`, or
`dylib` outputs.
`features` and `crate_features` may also use `select` with the same
host or target tokens.
Targets may also set `build_script` to compile and run a Cargo-style
build script before `rustc`; Once sets `OUT_DIR` for generated files.
Once consumes common build-script stdout directives:
`cargo:rustc-cfg`, `cargo:rustc-check-cfg`, `cargo:rustc-env`,
`cargo:rustc-link-arg`, `cargo:rustc-link-lib`, and
`cargo:rustc-link-search`. For crates with Cargo `links` metadata,
custom metadata from direct dependency build scripts is exposed to
downstream build scripts as `DEP_<LINKS>_<KEY>` environment variables.
Generated Cargo dependencies set `cap_lints = "allow"` so dependency
crates follow Cargo's lint-capping behavior.

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

Outputs land under `.once/out/<target>/`. The rule reference pages list
the exact outputs each rule emits.

## Prior Art

The Rust rule set follows the same provider model used by established
Rust build rules:

- Bazel Rust rules model Rust libraries as crate providers, pass direct
  dependencies through `--extern`, and make transitive artifacts
  available for downstream compilation.
- Buck2 Rust rules separate libraries, binaries, crate metadata,
  dependency contexts, and Cargo package lowering so third-party crates
  become normal graph nodes.

Once is not Buck-compatible, Bazel-compatible, or a drop-in
replacement for Cargo. The goal is an inspectable graph where agents
can discover rule contracts, materialize examples, and edit targets
through the same Once surfaces.
