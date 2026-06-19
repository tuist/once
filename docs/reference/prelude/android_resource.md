# `android_resource`

Android resource target.

## Description

Compiles Android resources into a static resource package with `aapt2`
and propagates assets to Android package targets. This is the preferred
place to declare shared `res/` and `assets/` trees. Android libraries
and binaries can depend on it through `deps`.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `manifest` | string | no | `AndroidManifest.xml` | Package-relative Android manifest |
| `resource_files` | list&lt;string&gt; | no | files under `res` | Android resource file glob patterns |
| `resource_dirs` | list&lt;string&gt; | no | `["res"]` | Resource roots passed to `aapt2 compile` |
| `assets` | list&lt;string&gt; | no | `[]` | Android asset file glob patterns |
| `asset_dirs` | list&lt;string&gt; | no | `["assets"]` | Asset roots propagated to Android package targets |
| `assets_dir` | string | no |  | Single package-relative asset root alias |
| `package` | string | no |  | Generated R package fallback |
| `namespace` | string | no |  | Java package for generated R classes |
| `custom_package` | string | no |  | Alias for generated R package |
| `compile_sdk` | int | no | highest installed | Android SDK API level used for android.jar |
| `min_sdk_version` | int | no | `23` | Minimum Android API level |
| `target_sdk_version` | int | no | `compile_sdk` | Target Android API level |
| `build_tools_version` | string | no | highest installed | Android SDK build-tools version |
| `android_sdk` | string | no | env | Android SDK root, otherwise `ANDROID_HOME` or `ANDROID_SDK_ROOT` |
| `aapt2` | string | no | SDK build-tools | Override `aapt2` path |

## Dep Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `android_resource` | Android resources merged into this resource package |

## Providers

The target emits `android_resource`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `default`, `resources` |

## Outputs

| Output | Location |
| --- | --- |
| Static resource package | `.once/out/<target>/resources.apk` |
| R source directory | `.once/out/<target>/generated/r` |
| R text symbols | `.once/out/<target>/R.txt` |

## Example

```toml
[[target]]
name = "SharedResources"
kind = "android_resource"

[target.attrs]
package = "dev.once.shared"
manifest = "AndroidManifest.xml"
resource_files = ["res/**"]
min_sdk_version = 23
```
