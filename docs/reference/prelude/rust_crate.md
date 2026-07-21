# `rust_crate`

Resolved third-party Cargo package lowered into a Rust library target.

## Description

Represents a sourced package from a resolved Cargo lockfile as a normal
Once graph target. `rust_crate` compiles to an rlib and emits the same
`rust_crate` provider shape consumed by `rust_library` and
`rust_binary` deps.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `package_name` | string | yes |  | Original Cargo package name |
| `version` | string | yes |  | Resolved Cargo package version |
| `crate_name` | string | no | target name | Rust crate name passed to `rustc`; `-` and `.` are rewritten as `_` when omitted |
| `crate_root` | string | no | `src/lib.rs` | Package-relative library root |
| `edition` | string | no | `2021` | Rust edition passed to `rustc` |
| `features` | list&lt;string&gt; | no | `[]` | Cargo feature names lowered to `--cfg=feature=...` flags |
| `crate_features` | list&lt;string&gt; | no | `[]` | Bazel-compatible alias for `features` |
| `target` | string | no | host target | Rust target triple passed to `rustc --target` |
| `env` | map&lt;string, string&gt; | no | `{}` | Environment variables for rustc, matching Buck2's `env` attribute |
| `rustc_env` | map&lt;string, string&gt; | no | `{}` | Bazel-compatible rustc environment variables |
| `rustc_env_files` | list&lt;string&gt; | no | `[]` | Files with `NAME=value` entries merged into the rustc environment before `env` and `rustc_env` |
| `rustc_flags` | list&lt;string&gt; | no | `[]` | Additional `rustc` flags appended after Once-managed flags |
| `cap_lints` | string | no | empty | Optional rustc lint cap passed as `--cap-lints`; generated Cargo dependencies use `allow` |
| `linker` | string | no | empty | Optional linker path passed as `-C linker=...`; defaults to `cc` for host Unix binary-like targets and to the Android NDK clang wrapper for Android targets when available |
| `linker_flags` | list&lt;string&gt; | no | `[]` | Additional linker flags lowered to `-C link-arg=...` |
| `native_linkopts` | list&lt;string&gt; | no | `[]` | Shared Rust compatibility attribute for native-producing Rust target kinds |
| `exported_linker_flags` | list&lt;string&gt; | no | `[]` | Buck-compatible alias for native linker flags propagated to downstream native consumers |
| `exported_post_linker_flags` | list&lt;string&gt; | no | `[]` | Buck-compatible propagated linker flags appended after normal exported linker flags |
| `linker_script` | string | no | empty | Package-relative linker script passed to the linker and included in the compile action inputs |
| `android_abi` | string | no | inferred | Android ABI directory for dynamic library outputs, such as `arm64-v8a` |
| `android_api` | int | no | `23` | Android API level used to select the NDK clang wrapper for Android targets |
| `android_ndk` | string | no | `ANDROID_NDK_HOME` | Android NDK root used to find clang wrapper linkers |
| `data` | list&lt;string&gt; | no | `[]` | Runtime data file globs propagated to downstream Rust binaries and tests |
| `compile_data` | list&lt;string&gt; | no | `[]` | Bazel-compatible compile-time data file globs included in the rustc action inputs |
| `crate_aliases` | map&lt;string, string&gt; | no | `{}` | Map dependency label, package name, or crate name to the local extern crate name |
| `aliases` | map&lt;string, string&gt; | no | `{}` | Bazel-compatible alias map from dependency label or crate name to local extern crate name |
| `named_deps` | map&lt;string, string&gt; | no | `{}` | Buck-compatible alias map from local extern crate name to dependency label or crate name |
| `cargo_package` | string | no | empty | Cargo package name used to select direct external deps from a `cargo_dependencies` dependency set. Defaults to `CARGO_PKG_NAME` when present |
| `build_script` | string | no | empty | Package-relative Cargo build script path run before `rustc`; common `cargo:rustc-*` stdout directives are consumed, dependency `cargo:rustc-link-search` outputs are replayed downstream, and direct dependency `links` metadata is consumed |
| `_build_script_inputs` | list&lt;string&gt; | resolver-owned | `[]` | Generated package source inputs made available to its Cargo build script |
| `source` | string | no |  | Cargo source identifier |
| `checksum` | string | no |  | Cargo.lock checksum for registry packages |

Accepted but unsupported attributes: `default_deps`, `doc_deps`, `doc_env`, `doc_link_style`,
`doc_linker_flags`, `doc_named_deps`, `link_deps`, `link_style`,
`mapped_srcs`, `proc_macro_deps`, `rpath`, `runtime_dependency_handling`,
and `rustdoc_flags`. Non-empty values under `[target.attrs]` fail validation.
Use the dependency roles with the same names under `[target.dependencies]`.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `rust_crate`, `rust_proc_macro`, `rust_dependency_set`, `c_provider` | Resolved Cargo package dependencies and C providers propagated to final link actions |
| `build_deps` | `rust_crate`, `rust_proc_macro`, `rust_dependency_set`, `c_provider` | Dependencies compiled for this package's Cargo build script |
| `proc_macro_deps` | `rust_proc_macro` | Procedural macros compiled for the execution host and passed to `rustc` through `--extern` |
| `link_deps` | `c_provider` | Native libraries and linker options propagated to final link actions |

## Sources

- [Cargo metadata](https://doc.rust-lang.org/stable/cargo/commands/cargo-metadata.html)
  defines the package targets, dependency kinds, features, and source metadata
  lowered into generated `rust_crate` targets.
- [The Cargo lockfile](https://doc.rust-lang.org/cargo/reference/lockfile.html)
  defines the resolved package versions and checksums attached to those targets.
- [Bazel rules for Rust, Crate Universe](https://bazelbuild.github.io/rules_rust/crate_universe_bzlmod.html)
  documents the upstream pattern of generating one build target per resolved
  crate while keeping dependency roles explicit.
- [Cargo build scripts](https://doc.rust-lang.org/cargo/reference/build-scripts.html)
  defines build dependencies and the directives consumed from build-script
  output.

## Providers

The target emits `rust_crate`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `rlib` |

## Outputs

| Output | Location |
| --- | --- |
| Rust library | `.once/out/<target>/lib<crate_name>-<metadata>.rlib` |

## Example

```toml
[[target]]
name = "itoa-1.0.14"
kind = "rust_crate"
srcs = ["vendor/itoa-1.0.14/src/**/*.rs"]

[target.attrs]
package_name = "itoa"
crate_name = "itoa"
version = "1.0.14"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "..."
crate_root = "vendor/itoa-1.0.14/src/lib.rs"
edition = "2021"
```
