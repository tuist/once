# `swift_android_library`

Swift shared library for Android.

## Description

Compiles Swift sources with `swiftc` into an Android `.so` and emits an
`android_native_library` provider. `android_binary` packages that provider
under `lib/<abi>/` in the APK. The rule is intentionally explicit about
Android because Swift cross compilation usually depends on a project-provided
Swift SDK, Android NDK sysroot, or extra compiler flags.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `android_abi` | string | no | inferred | Android ABI directory such as `arm64-v8a`, `armeabi-v7a`, `x86`, or `x86_64`. Inferred from common Android target triples when omitted |
| `target` | string | no | inferred | Swift target triple. Inferred from `android_abi` when omitted, or set directly when the ABI is inferable |
| `module_name` | string | no | target name | Swift module name |
| `sdk` | string | no |  | Optional sysroot passed to `swiftc -sdk` |
| `resource_dir` | string | no |  | Optional Swift resource directory passed to `swiftc -resource-dir` |
| `swiftc` | string | no | `PATH` | Override Swift compiler path |
| `swift_flags` | list&lt;string&gt; | no | `[]` | Additional Swift compiler flags |
| `linkopts` | list&lt;string&gt; | no | `[]` | Additional linker flags appended after Once-managed flags |

## Dep Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `swift_module`, `android_native_library`, `native_linkable` | Swift modules and Android native libraries linked or packaged with this library |

## Providers

The target emits `swift_module`, `android_native_library`, and
`native_linkable`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `default`, `native_library`, `swiftmodule` |

## Provider Record

| Field | Type | Meaning |
| --- | --- | --- |
| `label_id` | string | Canonical target id |
| `module_name` | string | Swift module name |
| `target` | string | Swift target triple passed to `swiftc` |
| `android_abi` | string | APK ABI directory |
| `swiftmodule_dir` | string | Directory holding the generated Swift module |
| `dylib` | string | Android shared library output |
| `android_native_libraries` | list&lt;record&gt; | Direct native libraries with `abi` and `path` fields |
| `transitive_android_native_libraries` | list&lt;record&gt; | Direct and dependency native libraries for APK packaging |
| `transitive_swiftmodule_dirs` | list&lt;string&gt; | Swift module search paths for downstream Swift Android libraries |

## Outputs

| Output | Location |
| --- | --- |
| Shared library | `.once/out/<target>/lib<module_name>.so` |
| Swift module | `.once/out/<target>/<module_name>.swiftmodule` |
| Swift doc | `.once/out/<target>/<module_name>.swiftdoc` |

## Example

```toml
[[target]]
name = "SharedSwift"
kind = "swift_android_library"
srcs = ["Sources/**/*.swift"]

[target.attrs]
android_abi = "arm64-v8a"
module_name = "SharedSwift"
sdk = "/opt/android-ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot"
swift_flags = ["-O"]
```
