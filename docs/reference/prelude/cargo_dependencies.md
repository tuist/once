# `cargo_dependencies`

Cargo dependency graph consumed by Rust targets.

## Description

During graph loading, Once reads a checked-in Cargo metadata snapshot or runs
`cargo metadata --locked --offline` against the configured Cargo manifest.
Every resolved registry or Git package becomes a synthetic
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
Graph loading rejects any provenance mismatch. First-party Rust targets
depend on this aggregate target and identify their Cargo package via
`CARGO_PKG_NAME` or the `cargo_package` attribute. Once then exposes only the
direct external dependencies Cargo reported for that workspace package.
The configured manifest and lockfile must both be covered by `resolver_inputs`,
together with any workspace member manifests Cargo needs to inspect. When
`resolver_inputs` is empty or omitted, `srcs` supplies those files instead.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `manifest` | string | no | `Cargo.toml` | Package-relative Cargo manifest path passed to `cargo metadata --manifest-path` |
| `lockfile` | string | no | `Cargo.lock` | Package-relative Cargo lockfile path read as a declared resolver input |
| `resolver_inputs` | list&lt;string&gt; | no | `srcs` | Package-relative text globs supplied to the resolver |
| `metadata_file` | string | no |  | Checked-in [JavaScript Object Notation](https://www.json.org/json-en.html) output from Cargo metadata with `once_snapshot` input and selection provenance |
| `host_metadata_file` | string | no |  | Cargo metadata snapshot with host selection provenance for the execution host. Required when `metadata_file` is combined with an explicit `target` |
| `vendor_dir` | string | no | `third_party/rust/vendor` | Package-relative directory containing vendored crate sources |
| `packages` | list&lt;string&gt; | no | `[]` | Optional package names to expose from this dependency set. Defaults to all resolved external packages |
| `features` | list&lt;string&gt; | no | `[]` | Cargo features passed to `cargo metadata --features` |
| `all_features` | bool | no | `false` | Pass `--all-features` to Cargo metadata |
| `no_default_features` | bool | no | `false` | Pass `--no-default-features` to Cargo metadata |
| `target` | string | no | host target | Rust target triple passed to Cargo as `--filter-platform` |
| `dep_rustc_flags` | list&lt;string&gt; | no | `[]` | Additional rustc flags applied to resolved crate builds. Panic strategy flags are stripped for procedural macro and host-tool crates so they keep the compiler's unwind strategy |
| `_cargo_resolved` | bool | resolver-owned | `false` | Marks an owner whose locked packages were expanded into graph targets |
| `_cargo_workspace_deps` | map&lt;string, list&lt;string&gt;&gt; | resolver-owned | `{}` | Records generated direct dependency target names by workspace package |
| `_cargo_workspace_dep_aliases` | map&lt;string, map&lt;string, string&gt;&gt; | resolver-owned | `{}` | Records Cargo dependency renames by workspace package and generated target |

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

## Checked Snapshot Example

The portable bundled starter omits `metadata_file` and resolves live in locked,
offline mode. This variant shows how to opt into a compiler-host-specific
checked snapshot:

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
`metadata_file`, graph loading runs Cargo in locked, offline mode. When a
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

Every generated package declares its complete vendored source tree as an input.
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
