# `rust_mobile_library`

Rust library compiled for Apple and Android native consumers.

## Description

Compiles one first-party Rust library target twice. The Apple compile emits a
`staticlib`; the Android compile emits a `cdylib`. Both compiles share the same
sources, crate metadata, features, Rust flags, and Rust dependencies, but they
use separate target triples, output directories, scratch files, and action
identifiers.

The target emits native provider fields for Apple and Android app targets. It
does not emit `rust_crate`, because there is no single rlib output for
downstream Rust compilation.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `crate_name` | string | no | target name | Rust crate name passed to `rustc`; `-` and `.` are rewritten as `_` when omitted |
| `crate_root` | string | no | `src/lib.rs` | Package-relative library root |
| `edition` | string | no | `2021` | Rust edition passed to `rustc` |
| `features` | list&lt;string&gt; | no | `[]` | Cargo feature names lowered to `--cfg=feature=...` flags |
| `crate_features` | list&lt;string&gt; | no | `[]` | Bazel-compatible alias for `features` |
| `apple_target` | string | yes |  | Rust target triple for the Apple static library compile |
| `android_target` | string | yes |  | Rust target triple for the Android shared library compile |
| `env` | map&lt;string, string&gt; | no | `{}` | Environment variables for `rustc`, matching Buck2's `env` attribute |
| `rustc_env` | map&lt;string, string&gt; | no | `{}` | Bazel-compatible `rustc` environment variables |
| `rustc_flags` | list&lt;string&gt; | no | `[]` | Additional `rustc` flags appended after Once-managed flags |
| `cap_lints` | string | no | empty | Optional `rustc` lint cap passed as `--cap-lints`; generated Cargo dependencies use `allow` |
| `linker_flags` | list&lt;string&gt; | no | `[]` | Additional linker flags lowered to `-C link-arg=...` |
| `native_linkopts` | list&lt;string&gt; | no | `[]` | Linker flags propagated to Apple app or framework targets |
| `android_abi` | string | no | inferred | Android [Application Binary Interface](https://developer.android.com/ndk/guides/abis) directory for the Android shared library output, such as `arm64-v8a` |
| `android_api` | int | no | `23` | Android platform level used to select the Android Native Development Kit clang wrapper |
| `android_ndk` | string | no | `ANDROID_NDK_HOME` | Android Native Development Kit root used to find clang wrapper linkers |
| `crate_aliases` | map&lt;string, string&gt; | no | `{}` | Map dependency label, package name, or crate name to the local extern crate name |
| `cargo_package` | string | no | empty | Cargo package name used to select direct external deps from a `cargo_dependencies` dependency set. Defaults to `CARGO_PKG_NAME` when present |
| `build_script` | string | no | empty | Package-relative Cargo build script path run before each compile; common `cargo:rustc-*` stdout directives are consumed, dependency `cargo:rustc-link-search` outputs are replayed downstream, and direct dependency `links` metadata is consumed |

## Dep Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `rust_crate`, `rust_proc_macro`, `rust_dependency_set` | Rust crate dependencies consumed through `--extern` |

## Providers

The target emits `native_linkable`, `apple_linkable`, and
`android_native_library`.

## Provider Record

| Field | Type | Meaning |
| --- | --- | --- |
| `label_id` | string | Original target label id |
| `archive` | string | Apple-linkable static library path |
| `staticlib` | string | Apple static library output |
| `transitive_archives` | list&lt;string&gt; | Archives consumed by Apple link targets |
| `transitive_linkopts` | list&lt;string&gt; | Native linker flags propagated to Apple link targets |
| `android_abi` | string | Android [Application Binary Interface](https://developer.android.com/ndk/guides/abis) for the shared library output |
| `dylib` | string | Android shared library output |
| `android_native_libraries` | list&lt;record&gt; | Direct Android native libraries with `abi` and `path` fields |
| `transitive_android_native_libraries` | list&lt;record&gt; | Direct and dependency Android native libraries packaged into Android applications |
| `transitive_sources` | list&lt;string&gt; | Rust sources from this target and Rust deps |

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `library` |

## Outputs

| Output | Location |
| --- | --- |
| Apple static library | `.once/out/<target>/apple/lib<crate_name>.a` or `.lib` |
| Android shared library | `.once/out/<target>/android/lib<crate_name>.so` |

## Example

```toml
[[target]]
name = "SharedRust"
kind = "rust_mobile_library"
srcs = ["shared/src/**/*.rs"]

[target.attrs]
crate_name = "shared"
crate_root = "shared/src/lib.rs"
edition = "2021"
apple_target = "aarch64-apple-ios-sim"
android_target = "aarch64-linux-android"
android_abi = "arm64-v8a"
android_api = 24
```
