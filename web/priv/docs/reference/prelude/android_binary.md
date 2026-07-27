# `android_binary`

Android APK target.

## Description

Builds an Android APK from Java and Kotlin sources, Android resources,
native shared libraries, `android_resource` deps, and `android_library`
deps. The target kind links resources with `aapt2`, compiles Java sources
with `javac`, compiles Kotlin sources with `kotlinc`, dexes runtime jars
with `d8`, packages dex and native libraries into the APK, zipaligns it,
and signs it with a debug key by default.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `application_id` | string | yes |  | Android application id |
| `manifest` | string | no | `AndroidManifest.xml` | Package-relative Android manifest |
| `resource_files` | list&lt;string&gt; | no | files under `res` | Android resource file glob patterns |
| `resource_dirs` | list&lt;string&gt; | no | `["res"]` | Resource roots passed to `aapt2 compile` |
| `assets` | list&lt;string&gt; | no | `[]` | Android asset file glob patterns |
| `asset_dirs` | list&lt;string&gt; | no | `["assets"]` | Asset roots packaged into the APK |
| `assets_dir` | string | no |  | Single package-relative asset root alias |
| `namespace` | string | no | `application_id` | Java package for generated R classes |
| `custom_package` | string | no |  | Alias for generated R package |
| `compile_sdk` | int | no | highest installed | Android SDK API level used for android.jar |
| `min_sdk_version` | int | no | `23` | Minimum Android API level |
| `target_sdk_version` | int | no | `compile_sdk` | Target Android API level |
| `version_code` | int | no | `1` | APK versionCode passed to `aapt2` |
| `version_name` | string | no | `1.0` | APK versionName passed to `aapt2` |
| `signing` | string | no | `debug` | `debug` for debug signing or `none` for unsigned output |
| `debug_keystore` | string | no | local debug keystore | Optional package-relative debug keystore |
| `debug_keystore_password` | string | no | `android` | Fixed public debug signing password |
| `debug_key_alias` | string | no | `androiddebugkey` | Key alias for debug signing only |
| `adb` | string | no | SDK platform-tools | Override adb path for `run` |
| `adb_serial` | string | no |  | Device serial passed to `adb -s` |
| `emulator` | string | no | Android software development kit emulator | Override Android emulator path used by visible runs |
| `emulator_device` | string | no |  | Android Virtual Device name started by `once run --visible` |
| `launch_activity` | string | no | launcher intent | Activity component launched by `once run` |
| `instruments` | target | no |  | Application target this package instruments when used by `android_instrumentation_test` |
| `build_tools_version` | string | no | highest installed | Android SDK build-tools version |
| `android_sdk` | string | no | env | Android SDK root, otherwise `ANDROID_HOME` or `ANDROID_SDK_ROOT` |
| `java_language_level` | string | no | `17` | Java source and target level passed to `javac` |
| `javac_opts` | list&lt;string&gt; | no | `[]` | Additional `javac` flags |
| `javacopts` | list&lt;string&gt; | no | `[]` | Bazel-compatible alias for additional `javac` flags |
| `kotlinc_opts` | list&lt;string&gt; | no | `[]` | Additional `kotlinc` flags |
| `dexopts` | list&lt;string&gt; | no | `[]` | Additional `d8` flags |

Tool override attrs are also available for `javac`, `jar`, `java`,
`java_home`, `kotlinc`, `kotlin_home`, `kotlin_stdlib`, `aapt2`, `d8`,
`apksigner`, `zipalign`, and `adb`.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `android_library`, `android_resource`, `android_native_library` | Android libraries, resources, and native shared libraries packaged into the APK |

## Providers

The target emits `android_application` and `android_apk`.

## Capabilities

| Capability | Output groups | Requires |
| --- | --- | --- |
| `build` | `default`, `apk`, `dex`, `resources` |  |
| `run` | `default` | `apk` |

## Source References

Target kind discovery returns these external concepts for partial graph
adoption:

- [`android_binary` from Bazel rules_android](https://bazelbuild.github.io/rules_android/#android_binary)
- [`android_binary` from Buck2](https://buck2.build/docs/prelude/rules/android/android_binary/)
- [`com.android.application` from the Android Gradle plugin](https://developer.android.com/build/configure-app-module)

Use them to inspect the source target that owns final application packaging,
then reproduce only its required dependency closure.

## Outputs

| Output | Location |
| --- | --- |
| APK | `.once/out/<target>/<name>.apk` |
| Debug keystore | `.once/out/<target>/debug.keystore` when `debug_keystore` is set |
| Unsigned APK | `.once/out/<target>/unsigned.apk` |
| Dex directory | `.once/out/<target>/dex` |
| Linked resource package | `.once/out/<target>/resources.apk` |

Native library deps are copied into the unsigned APK under
`lib/<abi>/<library>`. Providers such as
[`swift_android_library`](/reference/prelude/swift_android_library) and
[`rust_mobile_library`](/reference/prelude/rust_mobile_library) emit the
required `android_native_libraries` records.

## Signing

`signing = "debug"` stages and signs with `debug_keystore` when it is set.
When it is omitted, Once signs with `ANDROID_DEBUG_KEYSTORE` or
`~/.android/debug.keystore` in place. Once does not ship debug private key
material. The keystore SHA-256 is part of the signing action identity so
changing the local key invalidates cached signing output.

`debug_keystore_password` must stay `android`. Custom signing passwords are
not supported because action metadata and process arguments are not treated
as secret channels. `signing = "none"` leaves the APK unsigned after
zipalign. Production signing is not implemented yet.

## Running

`once run` first builds the APK because the `run` capability requires the
`apk` output group. The run action then executes direct `adb` commands:
wait for a device, install the APK with `adb install -r -d`, and launch the
app.

Pass `once run --visible` to request a visible runtime interface. When
`emulator_device` is set, Once starts that Android Virtual Device with the
Android emulator before waiting for a device. If `emulator_device` is empty,
visible runs use the already-connected physical device or emulator.

When `launch_activity` is empty, Once resolves the launcher activity on the
device with `cmd package resolve-activity --brief`, then starts the resolved
component with `am start -n`. When `launch_activity` is set, Once launches it
directly with `adb shell am start -n <component>`. Components may be written
as `package/.Activity`, `package/FullyQualifiedActivity`, or just an activity
name, which Once pairs with `application_id`.

Set `adb_serial` when more than one Android device or emulator is connected.

## Instrumentation

Set `instruments` on an `android_binary` test package to identify the app
under test. Then declare an
[`android_instrumentation_test`](/reference/prelude/android_instrumentation_test)
target that depends on both the app and the test package. The test target
installs both packages on a device or emulator and runs `am instrument`.

## Limitations

The target supports Java sources, Kotlin sources, resources, native shared
library packaging, debug signing, Android resource dependencies, Android
library dependencies, application package installation, and application
launch. Data binding,
manifest placeholder expansion, native splits,
shrinking, resource filtering, density filtering, no-compress packaging, and
startup profile packaging are not implemented yet. Non-empty values for
unsupported attributes fail validation instead of being ignored.

Use [`android_local_test`](/reference/prelude/android_local_test) for
host-side Java and Kotlin tests.

## Example

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
name = "Greeting"
kind = "android_library"
srcs = ["src/**/*.java"]
deps = ["./HelloResources"]

[target.attrs]
namespace = "dev.once.greeting"
manifest = "LibraryManifest.xml"
resource_files = []
min_sdk_version = 23

[[target]]
name = "Hello"
kind = "android_binary"
srcs = ["src/**/*.kt"]
deps = ["./Greeting"]

[target.attrs]
application_id = "dev.once.hello"
manifest = "AndroidManifest.xml"
resource_files = []
min_sdk_version = 23
version_code = 1
version_name = "1.0"
```
