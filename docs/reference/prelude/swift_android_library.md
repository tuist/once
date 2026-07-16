# `swift_android_library`

Swift shared library for Android.

## Description

Compiles Swift sources with `swiftc` into an Android `.so` and emits an
`android_native_library` provider. The Swift standard library is linked
statically. Once snapshots the matching C++ shared runtime from the Swift
Software Development Kit and `android_binary` packages both libraries under
the architecture-specific directory in the Android application package.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `android_abi` | string | no | inferred | Android ABI directory such as `arm64-v8a`, `armeabi-v7a`, `x86`, or `x86_64`. Inferred from common Android target triples when omitted |
| `android_api` | int | no | `28` | Android API level appended to API-less Android target triples |
| `target` | string | no | inferred | Swift target triple. Inferred from `android_abi` and `android_api` when omitted, or set directly when the ABI is inferable |
| `module_name` | string | no | target name | Swift module name |
| `package_name` | string | no | empty | Swift package name passed through `-package-name` when set |
| `sdk` | string | no |  | Optional sysroot passed to `swiftc -sdk` |
| `resource_dir` | string | no |  | Optional Swift resource directory passed to `swiftc -resource-dir` |
| `cxx_runtime` | string | no | Swift Software Development Kit sysroot | Optional absolute path to the C++ shared runtime packaged with the Swift library |
| `swift_sdk` | string | no | first Android SDK | Installed Swift SDK identifier used to discover default Android sysroot and Swift resource paths |
| `android_ndk` | string | no | `ANDROID_NDK_HOME` | Android NDK root used to find the LLVM tool directory |
| `tools_directory` | string | no | inferred | Directory containing Android clang and linker tools passed as `-tools-directory` |
| `swiftc` | string | no | `PATH` | Override Swift compiler path |
| `swift_flags` | list&lt;string&gt; | no | `[]` | Additional Swift compiler flags |
| `copts` | list&lt;string&gt; | no | `[]` | Bazel-compatible alias for additional Swift compiler flags |
| `defines` | list&lt;string&gt; | no | `[]` | Conditional compilation symbols lowered to `-D` flags and propagated to downstream Swift modules |
| `linkopts` | list&lt;string&gt; | no | `[]` | Additional linker flags appended after dependency linker flags and propagated downstream |
| `data` | list&lt;string&gt; | no | `[]` | Runtime data file globs propagated to downstream consumers |
| `swiftc_inputs` | list&lt;string&gt; | no | `[]` | Bazel-compatible extra Swift compiler input globs |
| `library_evolution` | bool | no | `false` | Enable Swift library evolution and emit a textual module interface |

Compatibility attributes declared for Bazel parity but not implemented yet:
`always_include_developer_search_paths`, `alwayslink`,
`generated_header_name`, `generates_header`, `linkstatic`, `plugins`, and
`private_deps`. Non-empty values fail analysis.

## Dep Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `swift_module`, `android_native_library` | Swift modules and Android native libraries linked or packaged with this library |

## Providers

The target emits `swift_module`, `android_native_library`, and
`native_linkable`.

## Runtime Boundary

Once follows the official
[Swift Software Development Kit for Android packaging model](https://www.swift.org/documentation/articles/swift-sdk-for-android-getting-started.html):
the Swift standard library is linked statically, while `libc++_shared.so` is
snapshotted from the selected toolchain, content-verified, cached, and emitted
as a native dependency. The complete runtime closure therefore reaches the
Android application package without a shell staging command.

Kotlin or Java callers still need a
[Java Native Interface](https://developer.android.com/training/articles/perf-jni)
boundary. The runnable shared-code starter includes direct entry points for
its primitive example. Richer application programming interfaces should use
the official [SwiftJava generator](https://github.com/swiftlang/swift-java),
whose Java Native Interface mode supports Android.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `default`, `native_library`, `swiftmodule` |

## Source References

Target kind discovery exposes the official
[Swift Android Software Development Kit guide](https://www.swift.org/documentation/articles/swift-sdk-for-android-getting-started.html)
for runtime packaging and the
[SwiftJava project](https://github.com/swiftlang/swift-java) for generated
Java Native Interface bindings. A coding harness can fetch either source
through Once before adapting a richer interoperation boundary.

## Provider Record

| Field | Type | Meaning |
| --- | --- | --- |
| `label_id` | string | Canonical target id |
| `module_name` | string | Swift module name |
| `target` | string | Swift target triple passed to `swiftc` |
| `android_abi` | string | APK ABI directory |
| `swiftmodule_dir` | string | Directory holding the generated Swift module |
| `swiftmodule` | string | Generated Swift module artifact |
| `swiftdoc` | string | Generated Swift documentation artifact |
| `swiftinterface` | string | Generated textual module interface when library evolution is enabled |
| `dylib` | string | Android shared library output |
| `android_native_libraries` | list&lt;record&gt; | Direct native libraries with `abi` and `path` fields |
| `transitive_android_native_libraries` | list&lt;record&gt; | Direct and dependency native libraries for APK packaging |
| `transitive_swiftmodule_dirs` | list&lt;string&gt; | Swift module search paths for downstream Swift Android libraries |
| `transitive_swiftmodule_inputs` | list&lt;string&gt; | Swift module artifacts declared as downstream compiler inputs |
| `transitive_swift_defines` | list&lt;string&gt; | Conditional compilation symbols propagated downstream |
| `transitive_linkopts` | list&lt;string&gt; | Linker options propagated downstream |
| `transitive_data` | list&lt;string&gt; | Runtime data propagated downstream |

## Outputs

| Output | Location |
| --- | --- |
| Shared library | `.once/out/<target>/lib<module_name>.so` |
| C++ shared runtime | `.once/out/<target>/libc++_shared.so` |
| Swift module | `.once/out/<target>/<module_name>.swiftmodule` |
| Swift doc | `.once/out/<target>/<module_name>.swiftdoc` |
| Swift interface | `.once/out/<target>/<module_name>.swiftinterface` when library evolution is enabled |

## Example

```toml
[[target]]
name = "SharedSwift"
kind = "swift_android_library"
srcs = ["Sources/**/*.swift"]

[target.attrs]
android_abi = "arm64-v8a"
module_name = "SharedSwift"
swift_sdk = "swift-6.3.2-RELEASE_android"
android_ndk = "/opt/android-ndk"
swift_flags = ["-O"]
```
