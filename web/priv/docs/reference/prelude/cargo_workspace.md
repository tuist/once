# `cargo_workspace`

Native Cargo project seed.

## Description

`cargo_workspace` runs `cargo metadata --locked --offline` and emits ordinary
first-party and external Rust targets. First-party packages become
`rust_library`, `rust_binary`, `rust_test`, or `rust_proc_macro` targets.
Locked external packages use the same fine-grained lowering as
`cargo_dependencies`.

Cargo remains authoritative for workspace membership, targets, features,
renamed dependencies, build scripts, versions, and checksums. External package
sources must already exist in the configured vendored source directory.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `manifest` | string | no | `Cargo.toml` | Package-relative Cargo manifest |
| `lockfile` | string | no | `Cargo.lock` | Package-relative authoritative lockfile |
| `resolver_inputs` | list&lt;string&gt; | no | `srcs` | Text inputs available while deriving the graph |
| `metadata_file` | string | no |  | Optional checked Cargo metadata snapshot |
| `host_metadata_file` | string | no |  | Optional host metadata snapshot for cross-compilation |
| `vendor_dir` | string | no | `vendor` | Package-relative vendored external sources |
| `features` | list&lt;string&gt; | no | `[]` | Selected Cargo features |
| `all_features` | bool | no | `false` | Select every Cargo feature |
| `no_default_features` | bool | no | `false` | Disable default Cargo features |
| `target` | string | no | host | Destination Rust target triple |
| `dep_rustc_flags` | list&lt;string&gt; | no | `[]` | Additional flags for external packages |

## Providers

The target emits `cargo_workspace`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | none |

## Direct Use

Discover and preview the native project:

```sh
once query native-projects
once query native-project cargo
```

Initialize the seed:

```sh
once edit init-native-project cargo
```

The imported target is equivalent to:

```toml
[[target]]
name = "cargo"
kind = "cargo_workspace"
srcs = ["Cargo.toml"]

[target.attrs]
resolver_inputs = ["Cargo.toml", "Cargo.lock", "**/Cargo.toml", ".cargo/config", ".cargo/config.toml"]
```

## Sources

- [Cargo metadata](https://doc.rust-lang.org/stable/cargo/commands/cargo-metadata.html)
  defines the native project graph.
- [The Cargo lockfile](https://doc.rust-lang.org/cargo/reference/lockfile.html)
  defines resolved versions and checksums.
