# `apple_framework`

Apple framework bundle.

## Description

Builds an Apple framework product with module metadata, a generated
`Info.plist` property-list file, and ad-hoc signing. Attributes whose names
contain `sdk` configure the
[Apple software development kit (SDK)](https://developer.apple.com/documentation/xcode)
used for the build.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `platform` | string | yes |  | Apple platform for the framework |
| `minimum_os` | string | no | `13.0` | Minimum supported operating system version |
| `target_sdk_version` | string | no | `minimum_os` | Software development kit version used in the target triple |
| `sdk_variant` | string | no | `simulator` | `simulator` or `device`; ignored on macOS (not configurable) |
| `xcode_developer_dir` | string | no | active Xcode | Xcode developer directory used to resolve build tools |
| `bundle_id` | string | no | `dev.once.<product_name>` | Framework bundle identifier |
| `product_name` | string | no | target name | Framework product name (not configurable) |
| `module_name` | string | no | `product_name` | Swift module name |
| `headers` | list&lt;string&gt; | no | `[]` | Headers packaged with the framework |
| `exported_headers` | list&lt;string&gt; | no | `[]` | Headers exported to downstream consumers |
| `resources` | list&lt;string&gt; | no | `[]` | Resource glob patterns bundled into the framework |
| `asset_catalogs` | list&lt;string&gt; | no | `[]` | Asset catalog paths compiled into the framework bundle |
| `privacy_manifest` | string | no |  | Privacy manifest placed in the framework bundle |
| `sdk_frameworks` | list&lt;string&gt; | no | `[]` | Apple software development kit frameworks linked by name |
| `weak_sdk_frameworks` | list&lt;string&gt; | no | `[]` | Apple software development kit frameworks linked weakly |
| `sdk_dylibs` | list&lt;string&gt; | no | `[]` | Apple software development kit dynamic libraries linked by name |
| `linkopts` | list&lt;string&gt; | no | `[]` | Extra linker flags |
| `swift_flags` | list&lt;string&gt; | no | `[]` | Extra Swift compiler flags |

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `apple_linkable`, `apple_resource`, `apple_swift_plugin`, `native_linkable` | Libraries, resources, native linkables, and Swift compiler plugins linked or embedded by the framework |

## Providers

The target emits `apple_linkable`, `apple_framework`, and
`apple_bundle`.

The provider separates link-time and runtime framework closures. A downstream
link action links this framework, while the final application or test bundle
receives every framework needed at runtime. Static archives consumed by this
framework stop at the dynamic link boundary and are not linked into the final
binary again.

If a final link also reaches one of those absorbed archives through a separate
static dependency, or two dynamic frameworks absorb the same archive, analysis
reports the frameworks and duplicate archive, then explains how to repair the
graph instead of producing two copies of the same code.

| Field | Type | Meaning |
| --- | --- | --- |
| `framework_path` | string | Built framework directory |
| `framework_module_name` | string | Module name used by direct consumers |
| `framework_files` | list&lt;string&gt; | Framework outputs tracked by the action graph |
| `transitive_archives` | list&lt;string&gt; | Empty after the dynamic link boundary |
| `absorbed_static_archives` | list&lt;string&gt; | Static archives already linked into this framework |
| `transitive_link_framework_bundles` | list&lt;record&gt; | Framework bundles a downstream link action must link directly |
| `transitive_framework_bundles` | list&lt;record&gt; | De-duplicated runtime framework closure with paths, module names, files, and owning targets |
| `transitive_frameworks` | list&lt;string&gt; | Compatibility view of runtime framework paths |

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `default`, `framework`, `dsyms`, `swiftmodule` |

## Outputs

| Output | Location |
| --- | --- |
| Framework bundle | `.once/out/<target>/<product_name>.framework` |
| Dynamic library | `.once/out/<target>/<product_name>.framework/<product_name>` |
| Swift module | `.once/out/<target>/<product_name>.framework/Modules/<module_name>.swiftmodule` |
| Swift documentation | `.once/out/<target>/<product_name>.framework/Modules/<module_name>.swiftdoc` |
| Module map | `.once/out/<target>/<product_name>.framework/Modules/module.modulemap` |
| Property list | `.once/out/<target>/<product_name>.framework/Info.plist` |
| Code signature | `.once/out/<target>/<product_name>.framework/_CodeSignature/CodeResources` |

An application or test only needs to depend on the framework it uses directly.
Once carries nested dynamic framework dependencies to the final bundle,
de-duplicates them by framework path, embeds each bundle once, and signs the
result.

## Limitations

Header packaging, resource bundling, asset catalogs, and privacy manifests are
unsupported. The target rejects non-empty values for those attributes instead
of ignoring them.

## Example

```toml
[[target]]
name = "UI"
kind = "apple_framework"
srcs = ["UI/Sources/*.swift"]

[target.attrs]
platform = "ios"
minimum_os = "17.0"
sdk_variant = "simulator"
bundle_id = "dev.once.UI"
```
