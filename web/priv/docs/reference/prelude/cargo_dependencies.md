# `cargo_dependencies`

Cargo dependency graph consumed by Rust targets.

## Description

During graph loading, Once reads a checked-in Cargo metadata snapshot or runs
`cargo metadata --locked` against the configured Cargo manifest. Cargo fetches
missing locked registry or Git sources into its local cache. No separate
`cargo vendor` or dependency-preparation command is required.
Every resolved registry, Git, or nonmember path package becomes a synthetic
`rust_crate` or `rust_proc_macro` target. Normal dependencies remain `deps`,
while packages needed by Cargo build scripts use the `build_deps` role.

The `cargo_dependencies` target does not compile those packages itself. It
aggregates the providers from the generated target graph, so independent
packages can compile concurrently and Cargo's dependency topology remains
visible to queries, validation, caching, and scheduling.

`Cargo.toml` and `Cargo.lock` stay the source of truth. Live resolution is
locked and fails when the lockfile does not match the manifest. Every external
package in checked-in metadata is cross-checked against an exact lock entry.
The snapshot must also carry `once_snapshot` provenance that binds every other
resolver input, the selected features and target, and the compiler host triple.
Graph loading rejects any provenance mismatch. Checked snapshots require an
explicit `vendor_dir`; Once never compiles snapshot host paths for this target. First-party Rust targets
depend on this aggregate target and identify their Cargo package via
`CARGO_PKG_NAME` or the `cargo_package` attribute. Once then exposes only the
direct external dependencies Cargo reported for that workspace package.
A `rust_test` for that package also links the package's direct external
dev-dependencies, and a first-party target that declares a `build_script`
links the package's direct external build-dependencies into that script, so
authored test and build-script targets resolve the same crates Cargo would.
The configured manifest and an existing lockfile must both be covered by
`resolver_inputs`, together with any workspace member manifests Cargo needs to
inspect. A missing or undeclared lockfile fails before Cargo runs. Unlike native
`cargo_workspace` discovery, this dependency target does not create a lockfile. When
`resolver_inputs` is empty or omitted, `srcs` supplies those files instead.
When the declared inputs include `.cargo/config.toml` or `.cargo/config`, Once
passes that file to Cargo explicitly. Vendored source replacement therefore
works even when Once loads the graph from a different process directory.

Generated target names include the destination target triple when `target` is
set. Native and cross-compiled dependency graphs can therefore coexist in one
package without synthetic target collisions.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `manifest` | string | no | `Cargo.toml` | Package-relative Cargo manifest path passed to `cargo metadata --manifest-path` |
| `lockfile` | string | no | `Cargo.lock` | Package-relative Cargo lockfile path read as a declared resolver input |
| `resolver_inputs` | list&lt;string&gt; | no | `srcs` | Package-relative text globs supplied to the resolver |
| `metadata_file` | string | no |  | Checked-in [JavaScript Object Notation](https://www.json.org/json-en.html) output from Cargo metadata with `once_snapshot` input and selection provenance |
| `host_metadata_file` | string | no |  | Cargo metadata snapshot with host selection provenance for the execution host. Required when `metadata_file` is combined with an explicit `target` |
| `vendor_dir` | string | with `metadata_file` |  | Package-relative directory containing pre-vendored crate sources. For live resolution, omit or leave empty to snapshot locked sources from Cargo's local cache into target outputs |
| `packages` | list&lt;string&gt; | no | `[]` | Optional package names to expose from this dependency set. Defaults to all resolved external packages |
| `features` | list&lt;string&gt; | no | `[]` | Cargo features passed to `cargo metadata --features` |
| `all_features` | bool | no | `false` | Pass `--all-features` to Cargo metadata |
| `no_default_features` | bool | no | `false` | Pass `--no-default-features` to Cargo metadata |
| `target` | string | no | host target | Rust target triple passed to Cargo as `--filter-platform` |
| `dep_rustc_flags` | list&lt;string&gt; | no | `[]` | Additional rustc flags applied to resolved crate builds. Panic strategy flags are stripped for procedural macro and host-tool crates so they keep the compiler's unwind strategy |
| `build_script_tools` | list&lt;string&gt; | no | `cmake`, `nasm`, `perl`, `pkg-config`, `protoc`, `python3` | Host tool names package build scripts may invoke; each is resolved on the search path during graph loading |
| `_cargo_resolved` | bool | resolver-owned | `false` | Marks an owner whose locked packages were expanded into graph targets |
| `_cargo_workspace_deps` | map&lt;string, list&lt;string&gt;&gt; | resolver-owned | `{}` | Records generated direct dependency target names by workspace package |
| `_cargo_workspace_dev_deps` | map&lt;string, list&lt;string&gt;&gt; | resolver-owned | `{}` | Records generated direct dev-dependency target names by workspace package, excluding packages that are also normal dependencies. Only `rust_test` targets link them |
| `_cargo_workspace_build_deps` | map&lt;string, list&lt;string&gt;&gt; | resolver-owned | `{}` | Records generated execution-host build-dependency target names by workspace package. Only targets that declare a `build_script` link them |
| `_cargo_workspace_dep_aliases` | map&lt;string, map&lt;string, string&gt;&gt; | resolver-owned | `{}` | Records Cargo dependency renames by workspace package and generated target |
| `_cargo_workspace_dev_dep_aliases` | map&lt;string, map&lt;string, string&gt;&gt; | resolver-owned | `{}` | Records Cargo dev-dependency renames by workspace package and generated target |
| `_cargo_workspace_build_dep_aliases` | map&lt;string, map&lt;string, string&gt;&gt; | resolver-owned | `{}` | Records Cargo build-dependency renames by workspace package and generated target |

The underscore-prefixed attributes are part of the typed resolver contract.
They are produced by Once and should not be set in `once.toml`.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `rust_crate`, `rust_proc_macro` | Locked packages emitted by the resolver and aggregated into `rust_dependency_set` |

## Providers

The target emits `rust_dependency_set`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | none |

## Examples

### Live Resolution

For live resolution without vendoring, declare the manifests and lockfile and
omit `vendor_dir`:

```toml
[[target]]
name = "cargo_dependencies"
kind = "cargo_dependencies"
srcs = ["Cargo.toml", "Cargo.lock", "apps/*/Cargo.toml"]

[target.attrs]
manifest = "Cargo.toml"
lockfile = "Cargo.lock"
```

Once materializes each package's complete source tree through a cacheable action.
Independent crates remain separate graph targets, with separate compilation
cache keys. Cargo reuses its local source cache on subsequent graph loads.
Normal Cargo configuration applies, including registry mirrors, source
replacement, and offline mode configured with `[net] offline = true`.

Once snapshots complete package directories, including build or version-control
state stored inside nonmember path and Git packages. Keep local build outputs
outside those package directories, for example with Cargo's `target-dir`, and do
not store secrets there.

The previous implicit `vendor_dir = "third_party/rust/vendor"` default no longer
applies. Set that attribute explicitly to retain pre-vendored resolution. A
checked snapshot without `vendor_dir` now reports a repair instead of silently
using absolute paths from the machine that generated the snapshot.

### Checked Snapshot

The portable bundled starter uses live locked resolution with explicit vendored
sources so it can run without network access. This variant shows how to opt into
a compiler-host-specific checked snapshot:

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

The optional `packages` filter limits what the aggregate provider exposes. It
does not alter Cargo's version or feature resolution. Transitive dependencies
remain ordinary generated graph edges of the selected package.

Regenerate the snapshot with the same features and target selection used by
the Once target whenever `Cargo.toml` or `Cargo.lock` changes. Without
`metadata_file`, graph loading runs Cargo in locked mode. When a
snapshot target sets `target`, also provide `host_metadata_file` so procedural
macros and build dependencies use the execution-host graph.

Augment each checked-in Cargo metadata object with an `once_snapshot` object:

```json
{
  "once_snapshot": {
    "inputs": {
      "Cargo.toml": "exact manifest text",
      "Cargo.lock": "exact lockfile text",
      ".cargo/config.toml": "exact Cargo configuration text",
      "apps/hello/Cargo.toml": "exact workspace member manifest text"
    },
    "selection": {
      "features": [],
      "all_features": false,
      "no_default_features": false,
      "target": "",
      "host": false,
      "host_triple": "x86_64-unknown-linux-gnu"
    }
  }
}
```

The input keys and contents must match every `resolver_inputs` file except the
target and host metadata snapshots. Set `host` to `true` only in
`host_metadata_file`. Set `target` to the target attribute value, including an
empty string when the target uses the execution host. Set `host_triple` to the
exact host reported by the selected Rust compiler for a native snapshot and for
every host metadata snapshot. Use an empty string for destination metadata when
`target` is explicit. This makes a manifest, configuration, feature, target, or
compiler host change fail during graph loading instead of silently reusing stale
metadata.

Every generated package declares its complete source tree as an input.
This covers files read through Rust source inclusion macros as well as Rust
sources, manifests, build scripts, licenses, and package data.

## Sources

- [Cargo metadata](https://doc.rust-lang.org/stable/cargo/commands/cargo-metadata.html)
  defines the package graph and the failure behavior of `--locked`.
- [The Cargo lockfile](https://doc.rust-lang.org/cargo/reference/lockfile.html)
  defines the resolved package versions and checksums Once preserves.
- [Bazel rules for Rust, Crate Universe](https://bazelbuild.github.io/rules_rust/crate_universe_bzlmod.html)
  documents the upstream pattern of translating a Cargo manifest and lockfile
  into generated build targets with distinct normal, procedural macro, and
  build-script dependencies.
