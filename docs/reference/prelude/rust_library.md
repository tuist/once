# `rust_library`

Rust rlib compiled with `rustc`.

## Description

Compiles a first-party Rust library target. Direct Rust deps are passed
through `--extern`; transitive rlibs are exposed as dependency search
paths for downstream Rust targets. The default output is an rlib, while
final artifact targets may set `crate_type` to `staticlib`, `cdylib`,
or `dylib`.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `crate_name` | string | no | target name | Rust crate name passed to `rustc`; `-` and `.` are rewritten as `_` when omitted |
| `crate_root` | string | no | `src/lib.rs` | Package-relative library root |
| `edition` | string | no | `2021` | Rust edition passed to `rustc` |
| `features` | list&lt;string&gt; | no | `[]` | Cargo feature names lowered to `--cfg feature=...` flags |
| `crate_features` | list&lt;string&gt; | no | `[]` | Bazel-compatible alias for `features` |
| `target` | string | no | host target | Rust target triple passed to `rustc --target` |
| `env` | map&lt;string, string&gt; | no | `{}` | Environment variables for rustc, matching Buck2's `env` attribute |
| `rustc_env` | map&lt;string, string&gt; | no | `{}` | Bazel-compatible rustc environment variables |
| `rustc_flags` | list&lt;string&gt; | no | `[]` | Additional `rustc` flags appended after Once-managed flags |
| `cap_lints` | string | no | empty | Optional rustc lint cap passed as `--cap-lints`; generated Cargo dependencies use `allow` |
| `linker` | string | no | empty | Optional linker path passed as `-C linker=...`; defaults to `cc` for host Unix binary-like targets and is omitted for cross targets unless set |
| `linker_flags` | list&lt;string&gt; | no | `[]` | Additional linker flags lowered to `-C link-arg=...` |
| `crate_aliases` | map&lt;string, string&gt; | no | `{}` | Map dependency label, package name, or crate name to the local extern crate name |
| `cargo_package` | string | no | empty | Cargo package name used to select direct external deps from a `cargo_dependencies` dependency set. Defaults to `CARGO_PKG_NAME` when present |
| `build_script` | string | no | empty | Package-relative Cargo build script path run before `rustc`; common `cargo:rustc-*` stdout directives and direct dependency `links` metadata are consumed |
| `crate_type` | string | no | `rlib` | Rust crate type for the output |

## Dep Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `rust_crate`, `rust_proc_macro`, `rust_dependency_set` | Rust crate dependencies consumed through `--extern` |

## Providers

The target emits `rust_crate`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `library` |

## Outputs

| Output | Location |
| --- | --- |
| Rlib | `.once/out/<target>/lib<crate_name>.rlib` |
| Static library | `.once/out/<target>/lib<crate_name>.a` or `.lib` |
| Dynamic library | `.once/out/<target>/lib<crate_name>.dylib`, `.so`, or `.dll` |

## Example

```toml
[[target]]
name = "hello"
kind = "rust_library"
srcs = ["src/**/*.rs"]

[target.attrs]
crate_name = "hello"
edition = "2021"
```
