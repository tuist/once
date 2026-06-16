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
| `source` | string | no |  | Cargo source identifier |
| `checksum` | string | no |  | Cargo.lock checksum for registry packages |

## Dep Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `rust_crate`, `rust_proc_macro`, `rust_dependency_set` | Resolved Cargo package dependencies |

## Providers

The target emits `rust_crate`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `rlib` |

## Outputs

| Output | Location |
| --- | --- |
| Rlib | `.once/out/<target>/lib<crate_name>.rlib` |

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
