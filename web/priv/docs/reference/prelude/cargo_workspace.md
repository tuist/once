# `cargo_workspace`

Native Cargo project seed.

## Description

`cargo_workspace` runs Cargo metadata and emits ordinary first-party and
external Rust targets. An existing `Cargo.lock` is used in locked mode. When it
is absent, Cargo resolves dependencies and creates it. First-party packages become
`rust_library`, `rust_binary`, `rust_test`, or `rust_proc_macro` targets.
Locked external packages use the same fine-grained lowering as
`cargo_dependencies`.

Cargo remains authoritative for workspace membership, targets, features,
renamed dependencies, build scripts, versions, and checksums. By default, Once
snapshots locked external package trees from Cargo's local cache into each
target's output. The repository stays unchanged. A pre-existing vendored source
directory can be selected explicitly.
Legacy lockfiles retain their checksum metadata without requiring a lockfile
format upgrade. Registry packages still require a locked checksum.
Feature resolution ignores Cargo terminal coloring, so presentation settings
do not change the imported feature set or action keys.
Targets whose `required-features` are not selected are omitted, matching
Cargo's target selection. Generated tests, benchmarks, and examples include
development dependencies. Procedural-macro unit tests retain access to the
compiler-provided `proc_macro` crate. Cargo doctests are not imported; run them
with Cargo separately. Each generated test runs from its own package root
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
| `build_script_sources` | map&lt;string, map&lt;string, string&gt;&gt; | no | `{}` | External consumer selector to sibling destination to locked package source file; see [sibling sources](/guide/graph/rust/#build-scripts-that-read-sibling-packages) |
| `features` | list&lt;string&gt; | no | `[]` | Selected Cargo features |
| `all_features` | bool | no | `false` | Select every Cargo feature |
| `no_default_features` | bool | no | `false` | Disable default Cargo features |
| `target` | string | no | host | Destination Rust target triple |
| `dep_rustc_flags` | list&lt;string&gt; | no | `[]` | Additional flags for external packages |
| `build_script_tools` | list&lt;string&gt; | no | `cmake`, `nasm`, `perl`, `pkg-config`, `protoc`, `python3` | Host tool names package build scripts may invoke; each is resolved on the search path during graph loading |
| `test_env` | map&lt;string, string&gt; | no | `{}` | Environment variables applied to generated tests after inherited variables |
| `test_env_inherit` | list&lt;string&gt; | no | `[]` | Host environment variable names explicitly inherited by generated tests |
| `test_cacheable` | bool | no | `true` | Cache successful generated test results; disable for tests depending on undeclared host tools or configuration |

## Providers

The target emits `cargo_workspace`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | none |

## Direct Use

On a fresh clone, the first graph load can access the network while Cargo
resolves or downloads dependencies. Once then imports the pinned graph from
`Cargo.lock` and Cargo's local source cache.

Inspect the automatically derived graph:

```sh
once query workspace
once query targets
```

No `once.toml` is required. To configure the resolver explicitly, author a
target equivalent to:

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

### Host-dependent tests

Generated tests use a restricted environment by default. Tests that invoke
Cargo or other host tools can opt into the required environment on the seed:

```toml
[target.attrs]
test_env_inherit = ["PATH", "CARGO_HOME", "RUSTUP_HOME"]
test_cacheable = false
```

Use `test_env` for explicit values, which take precedence over inherited
variables. Inheriting a tool search path does not declare the tools or their
external state as inputs, so keep test-result caching disabled for these
host-dependent suites. Compilation remains cacheable. These settings apply
to both integration tests and generated unit tests.

## Sources

- [Cargo metadata](https://doc.rust-lang.org/stable/cargo/commands/cargo-metadata.html)
  defines the native integration graph.
- [The Cargo lockfile](https://doc.rust-lang.org/cargo/reference/lockfile.html)
  defines resolved versions and checksums.
