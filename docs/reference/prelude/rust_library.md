# `rust_library`

Rust library compiled with `rustc`.

## Description

Compiles a first-party Rust library target. Direct Rust deps are passed
through `--extern`; transitive rlibs are exposed as dependency search
paths for downstream Rust targets. The default output is an rlib. Final
artifact targets may set `crate_type` to `staticlib`, `cdylib`, or
`dylib`. Static libraries expose Apple linkable provider fields, and
Android dynamic libraries expose APK native-library provider fields.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `crate_name` | string | no | target name | Rust crate name passed to `rustc`; `-` and `.` are rewritten as `_` when omitted |
| `crate_root` | string | no | `src/lib.rs` | Package-relative library root |
| `edition` | string | no | `2021` | Rust edition passed to `rustc` |
| `features` | list&lt;string&gt; | no | `[]` | Cargo feature names lowered to `--cfg=feature=...` flags |
| `crate_features` | list&lt;string&gt; | no | `[]` | Bazel-compatible alias for `features` |
| `target` | string | no | host target | Rust target triple passed to `rustc --target` |
| `env` | map&lt;string, string&gt; | no | `{}` | Environment variables for rustc, matching Buck2's `env` attribute |
| `rustc_env` | map&lt;string, string&gt; | no | `{}` | Bazel-compatible rustc environment variables |
| `rustc_flags` | list&lt;string&gt; | no | `[]` | Additional `rustc` flags appended after Once-managed flags |
| `cap_lints` | string | no | empty | Optional rustc lint cap passed as `--cap-lints`; generated Cargo dependencies use `allow` |
| `linker` | string | no | empty | Optional linker path passed as `-C linker=...`; defaults to `cc` for host Unix binary-like targets and to the Android NDK clang wrapper for Android targets when available |
| `linker_flags` | list&lt;string&gt; | no | `[]` | Additional linker flags lowered to `-C link-arg=...` |
| `native_linkopts` | list&lt;string&gt; | no | `[]` | Linker flags propagated to native consumers such as Apple app or framework targets |
| `android_abi` | string | no | inferred | Android ABI directory for `cdylib` or `dylib` outputs, such as `arm64-v8a` |
| `android_api` | int | no | `23` | Android API level used to select the NDK clang wrapper for Android targets |
| `android_ndk` | string | no | `ANDROID_NDK_HOME` | Android NDK root used to find clang wrapper linkers |
| `crate_aliases` | map&lt;string, string&gt; | no | `{}` | Map dependency label, package name, or crate name to the local extern crate name |
| `cargo_package` | string | no | empty | Cargo package name used to select direct external deps from a `cargo_dependencies` dependency set. Defaults to `CARGO_PKG_NAME` when present |
| `build_script` | string | no | empty | Package-relative Cargo build script path run before `rustc`; common `cargo:rustc-*` stdout directives are consumed, dependency `cargo:rustc-link-search` outputs are replayed downstream, and direct dependency `links` metadata is consumed |
| `crate_type` | string | no | `rlib` | Rust crate type for the output |

## Dep Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `rust_crate`, `rust_proc_macro`, `rust_dependency_set` | Rust crate dependencies consumed through `--extern` |

## Providers

The target emits `rust_crate`, `native_linkable`, `apple_linkable`, and
`android_native_library`. Native provider fields are populated only when
the selected `crate_type` produces a native artifact that the consumer can
use.

## Provider Record

| Field | Type | Meaning |
| --- | --- | --- |
| `rlib` | string | Rlib output when `crate_type = "rlib"` |
| `staticlib` | string | Static library output when `crate_type = "staticlib"` |
| `dylib` | string | Dynamic library output when `crate_type = "cdylib"` or `"dylib"` |
| `archive` | string | Apple-linkable archive path for `staticlib` outputs |
| `transitive_archives` | list&lt;string&gt; | Archives consumed by Apple link targets |
| `transitive_linkopts` | list&lt;string&gt; | Native linker flags propagated to Apple link targets |
| `android_abi` | string | Android ABI for dynamic library outputs |
| `android_native_libraries` | list&lt;record&gt; | Direct Android native libraries with `abi` and `path` fields |
| `transitive_android_native_libraries` | list&lt;record&gt; | Direct and dependency Android native libraries for APK packaging |

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `library` |

## Outputs

| Output | Location |
| --- | --- |
| Rust library | `.once/out/<target>/lib<crate_name>-<metadata>.rlib` |
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

```toml
[[target]]
name = "SharedRustApple"
kind = "rust_library"
srcs = ["src/**/*.rs"]

[target.attrs]
crate_name = "shared_rust"
crate_type = "staticlib"
target = "aarch64-apple-ios"

[[target]]
name = "SharedRustAndroid"
kind = "rust_library"
srcs = ["src/**/*.rs"]

[target.attrs]
crate_name = "shared_rust"
crate_type = "cdylib"
target = "aarch64-linux-android"
android_abi = "arm64-v8a"
```
