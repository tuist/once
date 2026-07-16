# `android_library`

Android library target.

## Description

Compiles Android Java and Kotlin sources with optional resources into a classes
jar, a static resource package, and an AAR. Downstream Android targets
consume the emitted jars, resource package, and any transitive native library
providers through the normal `deps` edge. Prefer a separate
`android_resource` target for reusable resources and assets.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `manifest` | string | no | `AndroidManifest.xml` | Package-relative Android manifest |
| `resource_files` | list&lt;string&gt; | no | files under `res` | Android resource file glob patterns |
| `resource_dirs` | list&lt;string&gt; | no | `["res"]` | Resource roots passed to `aapt2 compile` |
| `assets` | list&lt;string&gt; | no | `[]` | Android asset file glob patterns |
| `asset_dirs` | list&lt;string&gt; | no | `["assets"]` | Asset roots packaged into the AAR |
| `assets_dir` | string | no |  | Single package-relative asset root alias |
| `namespace` | string | no |  | Java package for generated R classes |
| `custom_package` | string | no |  | Alias for generated R package |
| `package` | string | no |  | Generated R package fallback |
| `compile_sdk` | int | no | highest installed | Android SDK API level used for android.jar |
| `min_sdk_version` | int | no | `23` | Minimum Android API level |
| `target_sdk_version` | int | no | `compile_sdk` | Target Android API level |
| `build_tools_version` | string | no | highest installed | Android SDK build-tools version |
| `android_sdk` | string | no | env | Android SDK root, otherwise `ANDROID_HOME` or `ANDROID_SDK_ROOT` |
| `java_language_level` | string | no | `17` | Java source and target level passed to `javac` |
| `javac_opts` | list&lt;string&gt; | no | `[]` | Additional `javac` flags |
| `javacopts` | list&lt;string&gt; | no | `[]` | Bazel-compatible alias for additional `javac` flags |
| `kotlinc_opts` | list&lt;string&gt; | no | `[]` | Additional `kotlinc` flags |
| `neverlink` | bool | no | `false` | Keep this library on compile classpaths while omitting its runtime dependency closure from application classpaths |

Tool override attrs are also available for `javac`, `jar`, `java`,
`java_home`, `kotlinc`, `kotlin_home`, `kotlin_stdlib`, and `aapt2`.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `android_library`, `android_resource`, `android_native_library` | Android libraries, resources, and native libraries consumed by this library |

## Providers

The target emits `android_library`, `android_archive`, and
`java_library`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `default`, `jar`, `aar`, `resources` |

## Source References

Target kind discovery returns these external concepts for partial graph
adoption:

- [`android_library` from Bazel rules_android](https://bazelbuild.github.io/rules_android/#android_library)
- [`android_library` from Buck2](https://buck2.build/docs/prelude/rules/android/android_library/)
- [`com.android.library` from the Android Gradle plugin](https://developer.android.com/build)

Use them when the requested dependency slice needs one reusable Android
library but does not need the complete source build.

## Outputs

| Output | Location |
| --- | --- |
| Classes jar | `.once/out/<target>/<name>.jar` |
| AAR | `.once/out/<target>/<name>.aar` |
| Static resource package | `.once/out/<target>/resources.apk` |
| R text symbols | `.once/out/<target>/R.txt` |

## Limitations

The target supports Java sources, Kotlin sources, Android resources, assets,
[Android Archive](https://developer.android.com/studio/projects/android-library)
packaging, Android resource dependencies, Android library dependencies, and
transitive native-library propagation. Android Interface Definition Language,
data binding, annotation processors, embedding native libraries directly into
Android Archives, and ProGuard consumer rules are not implemented yet.
Non-empty values for unsupported attributes fail validation instead of being
ignored.

## Example

```toml
[[target]]
name = "Greeting"
kind = "android_library"
srcs = ["src/**/*.kt"]

[target.attrs]
namespace = "dev.once.greeting"
manifest = "AndroidManifest.xml"
```
