# `rust_proc_macro`

Rust procedural macro compiled with `rustc`.

## Description

Compiles a Rust procedural macro for the execution host. Downstream
Rust targets consume the output through `--extern` as a procedural-macro
dependency instead of a normal target dependency.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `crate_name` | string | no | target name | Rust crate name passed to `rustc`; `-` and `.` are rewritten as `_` when omitted |
| `crate_root` | string | no | `src/lib.rs` | Package-relative macro root |
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
| `package_name` | string | no | target name | Original Cargo package name when lowered from Cargo metadata |
| `version` | string | no | empty | Resolved Cargo package version when lowered from Cargo metadata |
| `source` | string | no | empty | Cargo source identifier |
| `checksum` | string | no | empty | Cargo.lock checksum for registry packages |

Accepted but unsupported attributes: `default_deps`, `doc_deps`, `doc_env`, `doc_link_style`,
`doc_linker_flags`, `doc_named_deps`, `link_deps`, `link_style`,
`mapped_srcs`, `proc_macro_deps`, `rpath`, `runtime_dependency_handling`,
and `rustdoc_flags`. Non-empty values fail validation.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `rust_crate`, `rust_proc_macro`, `rust_dependency_set` | Rust crate dependencies consumed by the procedural macro |

## Providers

The target emits `rust_proc_macro`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `proc_macro` |

## Outputs

| Output | Location |
| --- | --- |
| Procedural macro library | `.once/out/<target>/lib<crate_name>.dylib`, `.so`, or `.dll` |

## Example

```toml
[[target]]
name = "identity"
kind = "rust_proc_macro"
srcs = ["src/**/*.rs"]

[target.attrs]
crate_name = "identity"
edition = "2021"
```
