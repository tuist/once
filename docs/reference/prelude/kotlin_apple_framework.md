# `kotlin_apple_framework`

Kotlin/Native framework for Apple platforms.

## Description

Compiles Kotlin sources with `kotlinc-native -produce framework` and emits an
Apple framework provider. Apple application, framework, and test bundle
targets consume the framework through their normal `deps` edge, add it to the
link line with `-framework`, and embed it where the bundle target supports
framework embedding.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `platform` | string | yes |  | Apple platform such as `ios`, `macos`, `tvos`, or `watchos` |
| `sdk_variant` | string | no | `simulator` | `simulator` or `device`; ignored for macOS target inference |
| `arch` | string | no | host arch | Target architecture used for Kotlin/Native target inference |
| `target` | string | no | inferred | Kotlin/Native target such as `ios_arm64`, `ios_simulator_arm64`, or `macos_arm64` |
| `product_name` | string | no | target name | Framework product name |
| `module_name` | string | no | product name | Swift/ObjC module name exported by the framework |
| `kotlinc_native` | string | no | `PATH` | Override `kotlinc-native` path |
| `kotlin_home` | string | no |  | Kotlin/Native installation root used to find `bin/kotlinc-native` |
| `java_home` | string | no | `JAVA_HOME` | Java runtime home exposed to `kotlinc-native` |
| `konan_data_dir` | string | no |  | Optional Kotlin/Native cache directory exposed as `KONAN_DATA_DIR` |
| `compiler_opts` | list&lt;string&gt; | no | `[]` | Additional `kotlinc-native` arguments |

## Dependency Edges

This target kind currently declares no dep edges.

## Providers

The target emits `kotlin_native_framework`, `apple_framework`,
`apple_bundle`, and `native_linkable`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `default`, `framework` |

## Provider Record

| Field | Type | Meaning |
| --- | --- | --- |
| `label_id` | string | Canonical target id |
| `platform` | string | Apple platform |
| `sdk_variant` | string | Apple SDK variant used for target inference |
| `arch` | string | Target architecture |
| `target` | string | Kotlin/Native target passed to the compiler |
| `framework_path` | string | Built framework directory |
| `framework_module_name` | string | Module name linked by Apple consumers |
| `framework_files` | list&lt;string&gt; | Framework file outputs tracked by the action graph |
| `transitive_link_framework_bundles` | list&lt;record&gt; | Framework bundles a downstream link action must link directly |
| `transitive_framework_bundles` | list&lt;record&gt; | Framework bundles a final application or test must embed and sign |
| `transitive_frameworks` | list&lt;string&gt; | Frameworks exposed to downstream Apple targets |

## Outputs

| Output | Location |
| --- | --- |
| Framework bundle | `.once/out/<target>/<product_name>.framework` |

## Example

```toml
[[target]]
name = "SharedKotlin"
kind = "kotlin_apple_framework"
srcs = ["Sources/**/*.kt"]

[target.attrs]
platform = "ios"
sdk_variant = "simulator"
arch = "arm64"
module_name = "SharedKotlin"
```
