# `cargo_dependencies`

Cacheable Cargo dependency set consumed by Rust targets.

## Description

Runs `cargo metadata --locked` against the configured Cargo manifest,
lowers resolved registry and git packages into cacheable Once Rust
crate actions, and exposes the direct external dependencies Cargo
reported for each workspace package.

`Cargo.toml` and `Cargo.lock` stay the source of truth. First-party
Rust targets depend on this target and identify their Cargo package via
`CARGO_PKG_NAME` or the `cargo_package` attribute.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `manifest` | string | no | `Cargo.toml` | Workspace-relative Cargo manifest path passed to `cargo metadata --manifest-path` |
| `lockfile` | string | no | `Cargo.lock` | Workspace-relative Cargo lockfile path included in the dependency action key |
| `vendor_dir` | string | no | `third_party/rust/vendor` | Workspace-relative directory containing vendored crate sources |
| `packages` | list&lt;string&gt; | no | `[]` | Optional package names to expose from this dependency set. Defaults to all resolved external packages |
| `features` | list&lt;string&gt; | no | `[]` | Cargo features passed to `cargo metadata --features` |
| `all_features` | bool | no | `false` | Pass `--all-features` to Cargo metadata |
| `no_default_features` | bool | no | `false` | Pass `--no-default-features` to Cargo metadata |
| `target` | string | no | host target | Rust target triple passed to Cargo as `--filter-platform` |
| `dep_rustc_flags` | list&lt;string&gt; | no | `[]` | Additional rustc flags applied to resolved crate builds. Panic strategy flags are stripped for procedural macro and host-tool crates so they keep the compiler's unwind strategy |

## Providers

The target emits `rust_dependency_set`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | none |

## Example

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
