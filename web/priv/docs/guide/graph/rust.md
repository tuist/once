---
prev: false
next: false
---

# Rust

Once can read an existing `Cargo.toml`, derive a typed build graph, and cache
each workspace package and locked dependency separately. You can build, run,
and test the project without first translating it into `once.toml`.

## Start With an Existing Cargo Project

From the directory that contains the root `Cargo.toml`, try the complete path
without creating `once.toml`:

```sh
once query targets
once build --ui
once test
```

The Runs interface shows the live build graph, including independently cached
package and dependency actions. Add `--ui` to `once test` to follow test
batches and results there too. The targetless test command selects first-party
tests and excludes test targets belonging only to external crates. Use `once
test --all` to request every test target in the resolved graph.

### Check the Toolchain

Once invokes `rustc` and `cargo` from the selected toolchain. Confirm that the
versions expected by the project are available:

```sh
rustc --version
cargo --version
```

If the repository uses [mise](https://mise.jdx.dev/) to pin them, install and
activate that configuration first:

```sh
mise install
mise exec -- rustc --version
mise exec -- cargo --version
```

Host binaries and tests also need the platform linker selected by the Rust
compiler. Cross-compiled and mobile outputs require the linker and Rust target
support for their destination platform.

### Resolve Dependencies

Once asks Cargo for package metadata while loading an automatically discovered
workspace. An existing `Cargo.lock` remains authoritative and is used with
Cargo's locked mode. When the lockfile is absent, Cargo resolves dependencies
and creates it before Once imports the resulting graph. The first load can
therefore access the network and populate Cargo's local source cache.

### Inspect the Derived Graph

To narrow the graph by target kind, inspect the first-party targets:

```sh
once query workspace
once query targets --kind rust_binary
once query targets --kind rust_library
once query targets --kind rust_test
```

No `once.toml` is required, and these commands do not write one.

The `cargo_workspace` seed runs Cargo metadata and emits
first-party libraries, binaries, procedural macros, unit and integration test
targets, build-script edges, and locked external packages.

The identifiers printed by `once query targets` are the source of truth.
Generated first-party identifiers carry their package and Cargo target role.
For a package named `hello` with a binary also named `hello`, discovery
normally derives `cargo_hello_bin_hello` and
`cargo_hello_bin_hello_unit_tests`.

Cargo workspaces use their shallowest matching `Cargo.toml`, so member
manifests do not create duplicate native integration seeds. Cargo remains
authoritative for workspace membership, default members, features, target
metadata, and resolved versions. Local path packages outside the workspace
remain dependency targets instead of becoming first-party workspace members.

Once snapshots external package trees from Cargo's local cache into target
outputs. It preserves files, executable modes, and symbolic links without
copying sources into the repository. It does not reuse or modify a repository's
`vendor` directory.

An explicit `vendor_dir` remains available for repositories that already
manage pre-vendored Cargo sources. With that attribute set, Once reads the
repository-managed source tree instead of Cargo's local source cache.

Targets gated by Cargo `required-features` appear only when every required
feature is selected. Generated tests, benchmarks, and examples include the
package's development dependencies, and hyphenated Cargo target names are
normalized for the Rust compiler automatically. Multi-output libraries expose
each declared Rust library crate type as a separate generated target.

Generated tests keep the rest of what Cargo gives a test. Each one runs from
its own package root, so a fixture opened through a package-relative path
resolves. Each one receives a `CARGO_BIN_EXE_<name>` entry for every binary in
its package, so a test that spawns the tool it exercises finds it. Each one
also receives the package's `CARGO_*` description.

Entries in the `[env]` table of Cargo configuration reach the compiler, the
build scripts, the test processes, and `once run`, so a repository that pins
something like its test thread count keeps that setting.

Packages compile once for the execution host. A separate host-only build of a
package appears only when it would actually differ: when a destination target
is requested, or when `dep_rustc_flags` carries a panic strategy that
procedural macros and build scripts must not inherit.

### Build, Run, and Test

The targetless commands are the quickest whole-workspace path. Use a generated
identifier when you want one artifact or test:

```sh
once build cargo_hello_bin_hello
once run cargo_hello_bin_hello
once test cargo_hello_bin_hello_unit_tests
```

Ask Once about a target before invoking it when its role is not obvious:

```sh
once query capabilities cargo_hello_bin_hello
```

Outputs are materialized under `.once/out/<target>/`. The
[`rust_binary` reference](/reference/prelude/rust_binary) and
[`rust_test` reference](/reference/prelude/rust_test) list their executable,
log, and test-result outputs.

### Keep Cargo Commands

Once can sit behind the `cargo` command for one native Cargo project or
workspace. Add this mise wrapper, then run `mise reshim` and activate mise in
the shell that starts the build:

```toml
[wrappers.cargo]
command = "once"
args = ["cargo", "--"]
```

Once routes the default debug build and test forms into the native graph:

```sh
cargo build
cargo test
```

`cargo build` builds the workspace seed and its resolved products. `cargo
test` runs each first-party test target once through Once, excluding tests
that belong to resolved external crates. `-q` or `--quiet`, `--locked`,
`--offline`, `--frozen`, and `--manifest-path Cargo.toml` are also supported.

For troubleshooting without mise, call the compatibility surface directly and
put the separator before the Cargo arguments:

```sh
once cargo -- test
```

Every other request, including release builds, feature selection, package
selection, cross-compilation targets, `cargo check`, `cargo run`, `cargo
clippy`, and a manifest path outside the workspace root, invokes the system
`cargo` executable with its original arguments and exit status. This preserves
Cargo behavior until the request has an exact Once equivalent.

### Confirm Caching

Run the same build twice without changing an input:

```sh
once build cargo_hello_bin_hello
once build cargo_hello_bin_hello
```

The second invocation should restore unchanged actions from the configured
cache. Changing one locked dependency invalidates that package and its
consumers. Changing one workspace package invalidates that package and its
dependants. Changing only a test file reruns the affected test without
recompiling an unchanged library.

Once caches compilation after Cargo resolves and materializes the sources
selected by `Cargo.lock`. Continue with [Caching](/guide/scripted/caching) to
configure a shared remote cache.

An explicitly authored seed can select Cargo features or a compilation target
without copying the generated package graph into the manifest:

```toml
[target.attrs]
features = ["workspace-package/feature-name"]
target = "aarch64-unknown-linux-gnu"
```

Use the [`cargo_workspace` reference](/reference/prelude/cargo_workspace) for
the complete seed contract.

### Workspaces and Troubleshooting

- If Cargo cannot resolve or download a source, fix the native Cargo error and
  retry the Once command.
- If a binary or example is absent, inspect its `required-features` and author
  a seed that selects those features explicitly.
- A build script that invokes a host tool needs that tool named in
  `build_script_tools`. The seed already lists the build tools that packages
  ending in `-sys` conventionally use, and each name is resolved on the search
  path while the graph loads. A script reaching for something else fails as an
  ordinary declared action until its tool is added.
- Test and run processes start with a cleared environment: a private `HOME`
  under the target's output directory, the standard system tool directories on
  the search path, and the variables described above. A test that reads the
  developer's real environment needs those variable names in `env_inherit` on
  an explicit target.
- A test that resolves `CARGO_BIN_EXE_<name>` with the `env!` macro captures
  the path while compiling, which pins it to the compiling action's execution
  root. That matches where the test runs for an ordinary local build but not
  under a sandbox policy or remote execution. Reading the variable at run time
  works in every mode.
- Cargo configuration files participate in graph resolution. Keep the
  repository's source replacement and target configuration available when
  loading the graph.

## Choose How Much Configuration to Own

Native discovery, manifests, and Starlark modules are composable layers. A
project does not need to migrate away from `Cargo.toml` to gain more control.

| Layer | Repository change | Use it when |
| --- | --- | --- |
| Native Cargo project | None | The graph derived from `Cargo.toml` already describes the build. |
| Explicit native seed | One `cargo_workspace` target in `once.toml` | The repository should configure features, target selection, caching, or execution infrastructure. |
| Explicit typed targets | Additional targets in `once.toml` | A package needs a custom boundary, cross-language dependency, mobile artifact, or operation that Cargo metadata does not describe. |
| Project Starlark module | A registered reusable target kind | Several targets need the same new behavior and an existing target kind cannot express it. |

Start with native discovery and stop there when it is sufficient. Add an
annotated script for a one-off repository task. Move data into `once.toml` when
the task needs typed dependencies, inputs, outputs, or platform selection. Move
behavior into a Starlark target kind only when the rule should be reusable.

An explicit `cargo_workspace` seed can live beside additional targets, so Cargo
can continue to own its package graph while Once owns only the exceptional
edges. Do not restate every generated Cargo package in `once.toml`.

When a custom target kind is necessary, keep project-specific Cargo behavior
inside that Starlark module and build it from generic action primitives. The
shared Rust execution layer remains independent of Cargo and other build
systems. The [Modules reference](/reference/modules/) covers target kind
schemas, resolvers, actions, validation, and module registration.

## Author a Graph When You Need More Control

Hand-authored targets are useful for a standalone Rust library without Cargo,
cross-language links, native mobile outputs, or a boundary that should be
cached independently.

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

## Keep Cargo Dependencies When Authoring Targets

Skip this section when the native `cargo_workspace` graph is sufficient. That
seed already imports the complete locked dependency graph. Use
`cargo_dependencies` when hand-authored Rust targets should keep Cargo as the
authority for third-party packages.

Keep third-party requirements in `Cargo.toml` and exact versions in
`Cargo.lock`. A root `cargo_dependencies` target lets Cargo resolve the
packages while Once builds the resolved crates as graph dependencies. The
bundled starter omits `metadata_file` and resolves live in locked, offline mode
so the same example remains portable across compiler hosts. To opt into a
checked snapshot instead, include it in the resolver inputs and set
`metadata_file`:

```toml
[[target]]
name = "cargo_dependencies"
kind = "cargo_dependencies"
srcs = [
  "Cargo.toml",
  "Cargo.lock",
  ".cargo/config.toml",
  "cargo-metadata.json",
  "apps/*/Cargo.toml",
]

[target.attrs]
manifest = "Cargo.toml"
lockfile = "Cargo.lock"
resolver_inputs = [
  "Cargo.toml",
  "Cargo.lock",
  ".cargo/config.toml",
  "cargo-metadata.json",
  "apps/*/Cargo.toml",
]
metadata_file = "cargo-metadata.json"
vendor_dir = "third_party/rust/vendor"
packages = ["itoa"]
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

With this configuration, the dependency target reads the checked-in Cargo
metadata snapshot while loading the graph. If `metadata_file` is omitted, it runs
`cargo metadata --locked --offline` instead.
Registry and Git packages come from the configured vendor directory. Workspace
and path packages remain first-party Once targets. The Cargo manifests and
lockfile stay authoritative for package names, versions, active features,
renamed dependencies, procedural macros, and build dependencies.
Every external metadata package must match an exact name, version, and source
entry in `Cargo.lock`; registry entries must also carry a lockfile checksum.
Checked-in metadata also carries `once_snapshot` provenance with the exact
resolver input text, feature and target selection, and the compiler host triple.
A manifest, configuration, feature, target, or compiler host change therefore
rejects stale metadata during graph loading. Once asks the selected Rust
compiler for its host triple before accepting a native snapshot.

For live offline metadata, configure Cargo to use the same vendored sources:

```toml
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "third_party/rust/vendor"
```

Keep that configuration in `resolver_inputs` with the manifests and lockfile.
Generated crate targets include the complete vendored package tree, which
covers files read through Rust source inclusion macros as well as package data.

Each resolved package becomes a synthetic `rust_crate` or `rust_proc_macro`
target. Normal Cargo edges use `deps`; build-script edges use the named
`build_deps` role. The `cargo_dependencies` target only aggregates their
providers, so Once can schedule independent crate builds concurrently instead
of compiling the locked package list inside one analysis implementation.

Inspect the imported packages, then build and run the first-party consumer:

```sh
once query targets --kind rust_crate
once query targets --kind rust_proc_macro
once build apps/hello/hello
once run apps/hello/hello
```

The bundled Cargo starter uses `itoa` from the locked graph and prints `42`.
That final run verifies more than graph loading: the local binary compiled and
linked against the provider emitted by the synthetic crate target.

Use [`once query schema cargo_dependencies`](/reference/prelude/cargo_dependencies)
before adding feature or target filters. For a cross-compiled binary, Once asks
Cargo for destination metadata and host metadata. Destination crates retain
the requested Rust target, while procedural macros, build dependencies, and
their required host variants compile for the execution host.

Refresh a native snapshot when dependency inputs change:

```sh
cargo metadata --format-version 1 --locked --offline > cargo-metadata.json
```

Use the same feature flags declared by `cargo_dependencies` and pass its target
through `--filter-platform`. Add the `once_snapshot` input and selection
provenance documented in the
[`cargo_dependencies` reference](/reference/prelude/cargo_dependencies). For
any snapshot target that sets `target`, record a second snapshot for the
execution host, mark its selection with `host = true`, and set
`host_metadata_file`.

## Use Build Scripts and Advanced Compiler Inputs

Rust targets can set `build_script` to compile and run a Cargo-style build
script before the main crate. Once provides `OUT_DIR` and consumes common
compiler configuration, environment, link argument, link library, and link
search directives printed by the script. Dependency link-search outputs and
Cargo `links` metadata are available to downstream targets and build scripts.

Rust libraries, binaries, tests, crates, and procedural macros can also depend
on `c_library` targets. Static and dynamic library paths plus transitive linker
options flow through intermediate Rust crates and are applied by the final
Rust link action. Native provider fields remain available to Apple and Android
consumers of Rust outputs.

Use named dependency roles when the relationship has compile-time semantics
that differ from an ordinary Rust crate:

```toml
[[target]]
name = "hello"
kind = "rust_binary"
srcs = ["src/main.rs"]
deps = ["./greeting"]

[target.dependencies]
proc_macro_deps = ["./derive_greeting"]
link_deps = ["./native_support"]
```

`proc_macro_deps` accepts `rust_proc_macro` providers built for the execution
host. `link_deps` accepts `c_provider` records and applies their libraries and
linker options to final artifacts. Existing targets may continue placing these
providers in `deps`, but named roles make the contract explicit and allow Once
to diagnose a provider in the wrong role before analysis.

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
deps = ["./SharedCore"]
srcs = ["src/shared/**/*.rs"]

[target.attrs]
crate_name = "shared_rust"
apple_target = "aarch64-apple-ios"
android_target = "aarch64-linux-android"
android_abi = "arm64-v8a"
android_api = 24

[[target]]
name = "SharedCore"
kind = "rust_mobile_library"
srcs = ["src/core/**/*.rs"]

[target.attrs]
crate_name = "shared_core"
apple_target = "aarch64-apple-ios"
android_target = "aarch64-linux-android"
android_abi = "arm64-v8a"
```

An Apple consumer requests a static library. An Android consumer requests a
shared library and packages it for the configured
[Application Binary Interface](https://developer.android.com/ndk/guides/abis).
Android linking requires the
[Android Native Development Kit](https://developer.android.com/ndk), found
through `ANDROID_NDK_HOME` or `android_ndk`.

Dependencies between `rust_mobile_library` targets are compiled recursively
for the platform requested by the Apple or Android consumer. Use explicit
platform-specific `rust_library` targets only when a dependency must expose a
host or single-target rlib instead of the deferred mobile provider.

## Supported Target Kinds and Limitations

Use the target kind reference for each role:

- [`rust_library`](/reference/prelude/rust_library)
- [`rust_binary`](/reference/prelude/rust_binary)
- [`rust_test`](/reference/prelude/rust_test)
- [`rust_proc_macro`](/reference/prelude/rust_proc_macro)
- [`cargo_workspace`](/reference/prelude/cargo_workspace)
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
