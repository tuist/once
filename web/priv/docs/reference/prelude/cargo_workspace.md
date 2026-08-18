# `cargo_workspace`

Native Cargo project seed.

## Description

`cargo_workspace` runs `cargo metadata --locked --offline` and emits ordinary
first-party and external Rust targets. First-party packages become
`rust_library`, `rust_binary`, `rust_test`, or `rust_proc_macro` targets.
Locked external packages use the same fine-grained lowering as
`cargo_dependencies`.

Cargo remains authoritative for workspace membership, targets, features,
renamed dependencies, build scripts, versions, and checksums. By default, Once
snapshots locked external package trees from Cargo's local cache into each
target's output. The repository stays unchanged. A pre-existing vendored source
directory can be selected explicitly.
Targets whose `required-features` are not selected are omitted, matching
Cargo's target selection. Generated tests, benchmarks, and examples include
development dependencies. Each generated test runs from its own package root
and receives a `CARGO_BIN_EXE_<name>` entry for every binary in its package,
matching what Cargo gives an integration test. Entries in the `[env]` table of
Cargo configuration reach the compiler, the build scripts, the test processes,
and `once run`.

Packages are compiled once for the execution host. A second, host-only build
of a package appears only when it would differ: when `target` names something
other than the host, or when `dep_rustc_flags` carries a panic strategy that
compiler plugins and build scripts must not inherit. Cargo target names are normalized to valid Rust crate
identifiers while binary names retain their manifest spelling. Multi-output
libraries emit one target per declared Rust library crate type. Cargo's
workspace member list determines which packages are first-party, so local path
dependencies outside that list remain ordinary dependency targets.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `manifest` | string | no | `Cargo.toml` | Package-relative Cargo manifest |
| `lockfile` | string | no | `Cargo.lock` | Package-relative authoritative lockfile |
| `resolver_inputs` | list&lt;string&gt; | no | `srcs` | Text inputs available while deriving the graph |
| `metadata_file` | string | no |  | Optional checked Cargo metadata snapshot |
| `host_metadata_file` | string | no |  | Optional host metadata snapshot for cross-compilation |
| `vendor_dir` | string | no | Once-managed | Optional package-relative pre-vendored external sources |
| `features` | list&lt;string&gt; | no | `[]` | Selected Cargo features |
| `all_features` | bool | no | `false` | Select every Cargo feature |
| `no_default_features` | bool | no | `false` | Disable default Cargo features |
| `target` | string | no | host | Destination Rust target triple |
| `dep_rustc_flags` | list&lt;string&gt; | no | `[]` | Additional flags for external packages |
| `build_script_tools` | list&lt;string&gt; | no | `cmake`, `nasm`, `perl`, `pkg-config`, `protoc`, `python3` | Host tool names package build scripts may invoke; each is resolved on the search path during graph loading |

## Providers

The target emits `cargo_workspace`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | none |

## Direct Use

On a fresh clone, populate Cargo's local source cache from the lockfile:

```sh
cargo fetch --locked
```

Once never performs that network operation while loading or building the
graph.

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

Keep this seed as the only Once target while Cargo metadata describes the
complete build. Add explicit targets beside it for exceptional cross-language
or packaging boundaries. Use a project Starlark module only when those
exceptions share reusable behavior that the built-in target kinds cannot
express.

## Sources

- [Cargo metadata](https://doc.rust-lang.org/stable/cargo/commands/cargo-metadata.html)
  defines the native project graph.
- [The Cargo lockfile](https://doc.rust-lang.org/cargo/reference/lockfile.html)
  defines resolved versions and checksums.
