# Android Graph

Once builds Android resources, Java libraries, and APKs from declarative
`once.toml` manifests. [`android_resource`](/reference/prelude/android_resource)
compiles `res/` trees with `aapt2` and propagates assets to package targets.
[`android_library`](/reference/prelude/android_library) compiles Java sources,
generates R classes, emits a classes jar, and packages an AAR. [`android_binary`](/reference/prelude/android_binary)
links resources, compiles Java sources, dexes runtime jars with `d8`, packages
an APK, zipaligns it, and signs it with a debug key by default.

For the per-target-kind attribute, dep, provider, and capability tables see
the [Prelude reference](/reference/prelude/).

## Targets

The preferred shape follows Buck2's separation between resources, libraries,
and final Android packages. Put reusable resources in an `android_resource`
target, then let libraries or binaries depend on it:

```toml
[[target]]
name = "HelloResources"
kind = "android_resource"

[target.attrs]
package = "dev.once.hello"
manifest = "ResourcesManifest.xml"
resource_files = ["res/**"]
min_sdk_version = 23

[[target]]
name = "Hello"
kind = "android_binary"
srcs = ["src/**/*.java"]
deps = ["./HelloResources"]

[target.attrs]
application_id = "dev.once.hello"
manifest = "AndroidManifest.xml"
min_sdk_version = 23
version_code = 1
version_name = "1.0"
```

Dependency references are root-relative by default. `./` and `../`
references resolve from the package that owns the manifest.

`android_library` can own Java sources and optional resources:

```toml
[[target]]
name = "Greeting"
kind = "android_library"
srcs = ["src/**/*.java"]

[target.attrs]
namespace = "dev.once.greeting"
manifest = "AndroidManifest.xml"
```

## Resources

`android_resource` is the guide-level default for shared Android
resources. It compiles matching resource files into `aapt2` compiled
resource units, links a static resource package, and propagates those
compiled units to consumers. `android_binary` merges dependency resource
units into the final APK link instead of treating static resource
packages as Android framework APKs.

The resource manifest should describe the resource package. The final
application manifest belongs on `android_binary`, where launcher
activities, app metadata, version values, and the application id are
resolved for the APK.

Inline `resource_files`, `resource_dirs`, `assets`, `asset_dirs`, and
`assets_dir` remain available on `android_library` and `android_binary`
for small targets and migration cases. For shared resources, prefer an
explicit `android_resource` dep because the provider edge keeps resource
ownership clear and queryable.

## Commands

Inspect the graph with [`once query`](/reference/cli/query):

```sh
once query targets --kind android_binary
once query capabilities apps/hello/Hello
once query schema android_resource
once query schema android_binary
```

Build an APK with [`once build`](/reference/cli/build):

```sh
once build apps/hello/Hello
```

Outputs land under `.once/out/<target>/`. The target kind reference pages list
the exact outputs each target kind emits.

## Running Apps

`android_binary` produces an APK and exposes `run`. `once run` first
materializes the required APK output, then executes the Android run action
declared by the target kind. That run action is not cacheable, so each
`once run` attempts a fresh install and launch instead of replaying an
action-cache hit.

The Android target kind owns the platform behavior. It waits for an Android
device or emulator through `adb`, installs the APK with `adb install -r -d`,
resolves the launcher activity on the device, and starts that component with
`am start`. Set `launch_activity` when the app needs an explicit activity
component. Set `adb_serial` when more than one device is connected.

```sh
once build apps/hello/Hello
once run apps/hello/Hello
```

## Toolchain

Android targets require a JDK with `java`, `javac`, `jar`, and `keytool`.
They also require the Android SDK command-line tools plus an installed
Android platform and build-tools package. Build-tools provide `aapt2`, `d8`,
`zipalign`, and `apksigner`. Running apps also requires platform-tools so
`adb` is available.

Android targets find the SDK from `android_sdk`, `ANDROID_HOME`, or
`ANDROID_SDK_ROOT`. When `compile_sdk` or `build_tools_version` is omitted,
the target kind picks the highest installed platform or build-tools version
under the SDK root.

Java-backed Android targets use `javac`, `jar`, and `java` from the host
toolchain unless those paths are overridden. The Android SDK tools also
receive `JAVA_HOME` when it is available, which keeps `d8` and `apksigner`
working with mise-managed Java installs.

Android build actions currently require a POSIX-compatible host shell for
file staging and directory hashing. App launch actions use direct `adb`
commands.

The current implementation supports Java sources, Android resources,
assets, static resource packages, AAR packaging, dexing, APK packaging,
zipalign, and debug signing. Kotlin, AIDL, data binding, annotation
processors, native libraries, instrumentation tests, manifest placeholder
expansion, shrinking, density filtering, no-compress packaging, startup
profiles, and production signing are not implemented yet. Non-empty
unsupported attributes fail analysis instead of being ignored.

## Configurable Attributes

Android targets support `select` values for configurable attributes.
The active Android configuration exposes tokens such as:

- `android`
- `compile_sdk_<api>`, such as `compile_sdk_35`
- `api_<api>`, such as `api_35`
- `min_sdk_<api>`, such as `min_sdk_23`
- `debug`
- `default`

Example:

```toml
[target.attrs]
javac_opts = { select = { android = ["-Xlint:deprecation"], default = [] } }
```

The attributes that decide the Android configuration, such as
`compile_sdk` and `min_sdk_version`, are not configurable. Attributes
marked non-configurable in the target kind schema also reject `select`.

## Prior Art

The Android target kind set uses both
[Bazel's Android rules](https://github.com/bazelbuild/rules_android) and
[Buck2's Android rules](https://github.com/facebook/buck2) as reference
points:

- Bazel's Android rules provide a broad compatibility vocabulary for
  Java sources, resources, assets, SDK selection, dexing, APK packaging,
  signing, and migration-friendly attribute names.
- Buck2's Android rules provide the cleaner shape for Once: Android
  resources are first-class packageable nodes, Java libraries focus on
  compilation and providers, and binaries own final resource linking,
  dexing, APK assembly, and signing.

Once is not Buck-compatible, Bazel-compatible, or a drop-in replacement
for Gradle. Android behavior lives in Starlark target kinds, while the
Rust side stays focused on generic graph loading, validation, providers,
actions, caching, and execution.
