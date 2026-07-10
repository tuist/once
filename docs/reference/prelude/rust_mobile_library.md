# `rust_mobile_library`

Rust library materialized by Apple and Android native consumers.

## Description

Describes one first-party Rust library target that Apple and Android consumers
can materialize for their platform. Apple consumers declare a `staticlib`
compile, and Android consumers declare a `cdylib` compile. Each consumer only
declares the variant it needs, so Android-only builds do not require the Apple
Rust target and Apple-only builds do not require the Android linker.

The target emits native provider fields for Apple and Android app targets. It
does not emit `rust_crate`, because there is no single rlib output for
downstream Rust compilation.

Rust dependencies are not supported on this target kind yet. Use explicit
platform-specific [`rust_library`](/reference/prelude/rust_library) targets
when the shared Rust code depends on other Rust crates.

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
| `rustc_env_files` | list&lt;string&gt; | no | `[]` | Files with `NAME=value` entries merged into the rustc environment before `env` and `rustc_env` |
| `rustc_flags` | list&lt;string&gt; | no | `[]` | Additional `rustc` flags appended after Once-managed flags |
| `cap_lints` | string | no | empty | Optional `rustc` lint cap passed as `--cap-lints`; generated Cargo dependencies use `allow` |
| `linker_flags` | list&lt;string&gt; | no | `[]` | Additional linker flags lowered to `-C link-arg=...` |
| `native_linkopts` | list&lt;string&gt; | no | `[]` | Linker flags propagated to Apple app or framework targets |
| `exported_linker_flags` | list&lt;string&gt; | no | `[]` | Buck-compatible alias for native linker flags propagated to downstream native consumers |
| `exported_post_linker_flags` | list&lt;string&gt; | no | `[]` | Buck-compatible propagated linker flags appended after normal exported linker flags |
| `linker_script` | string | no | empty | Package-relative linker script passed to each platform linker and included in compile action inputs |
| `android_abi` | string | no | inferred | Android [Application Binary Interface](https://developer.android.com/ndk/guides/abis) directory for the Android shared library output, such as `arm64-v8a` |
| `android_api` | int | no | `23` | Android platform level used to select the Android Native Development Kit clang wrapper |
| `android_ndk` | string | no | `ANDROID_NDK_HOME` | Android Native Development Kit root used to find clang wrapper linkers |
| `data` | list&lt;string&gt; | no | `[]` | Runtime data file globs propagated through each materialized platform provider |
| `compile_data` | list&lt;string&gt; | no | `[]` | Bazel-compatible compile-time data file globs included in each platform rustc action input set |
| `build_script` | string | no | empty | Package-relative Cargo build script path run before each platform compile |

## Dep Edges

This target kind currently declares no dependency edges.

## Providers

The target emits `native_linkable`, `apple_linkable`, and
`android_native_library`.

## Provider Record

| Field | Type | Meaning |
| --- | --- | --- |
| `label_id` | string | Original target label id |
| `transitive_sources` | list&lt;string&gt; | Rust sources from this target |
| `transitive_data` | list&lt;string&gt; | Runtime data propagated through materialized platform providers |

Apple consumers materialize `archive`, `staticlib`, `transitive_archives`,
and `transitive_linkopts` while collecting their link inputs. Android consumers
materialize `android_abi`, `dylib`, `android_native_libraries`, and
`transitive_android_native_libraries` while collecting native libraries.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | none |

## Outputs

| Output | Location |
| --- | --- |
| Apple static library | `.once/out/<consumer>/rust-mobile/<target>/apple/lib<crate_name>.a` or `.lib` |
| Android shared library | `.once/out/<consumer>/rust-mobile/<target>/android/lib<crate_name>.so` |

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
