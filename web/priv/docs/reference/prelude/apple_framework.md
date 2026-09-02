# `apple_framework`

Apple framework bundle.

## Description

Builds Swift, Objective-C, C, and C++ sources into a dynamic Apple framework
with module metadata, framework resources, a generated `Info.plist`
property-list file, and ad-hoc signing. Attributes whose names
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
| `exported_header_dirs` | list&lt;string&gt; | no | `[]` | Header search directories exported to downstream consumers |
| `private_header_dirs` | list&lt;string&gt; | no | `[]` | Header search directories used only while compiling the framework |
| `resources` | list&lt;string&gt; | no | `[]` | Resource glob patterns bundled into the framework |
| `structured_resources` | list&lt;string&gt; | no | `[]` | Resource directory roots whose own basename is preserved in the framework |
| `asset_catalogs` | list&lt;string&gt; | no | `[]` | Asset catalog paths compiled into the framework bundle |
| `privacy_manifest` | string | no |  | Privacy manifest placed in the framework bundle |
| `sdk_frameworks` | list&lt;string&gt; | no | `[]` | Apple software development kit frameworks linked by name |
| `weak_sdk_frameworks` | list&lt;string&gt; | no | `[]` | Apple software development kit frameworks linked weakly |
| `sdk_dylibs` | list&lt;string&gt; | no | `[]` | Apple software development kit dynamic libraries linked by name |
| `linkopts` | list&lt;string&gt; | no | `[]` | Extra linker flags |
| `swift_flags` | list&lt;string&gt; | no | `[]` | Extra Swift compiler flags |
| `clang_flags` | list&lt;string&gt; | no | `[]` | Extra Clang compiler flags |
| `per_source_clang_flags` | map&lt;string,string&gt; | no | `{}` | [JavaScript Object Notation](https://www.json.org/json-en.html)-encoded Clang flags keyed by source path |
| `defines` | list&lt;string&gt; | no | `[]` | Compatibility conditions passed to both Swift and Clang |
| `swift_defines` | list&lt;string&gt; | no | `[]` | Swift conditional compilation conditions |
| `clang_defines` | list&lt;string&gt; | no | `[]` | C-family preprocessor definitions |
| `enable_testing` | boolean | no | `false` | Makes internal Swift declarations available to dependent tests |
| `swift_testing` | boolean | no | `false` | Compiles sources that import the Swift Testing framework |
| `xctest_support` | boolean | no | `false` | Compiles sources that import the XCTest framework |
| `library_evolution` | boolean | no | `false` | Emits stable Swift module interfaces for binary compatibility |
| `emit_dsym` | boolean | no | `false` | Emits debug information for symbol bundles |
| `archs` | list&lt;string&gt; | no | host architecture | Target architectures (not configurable) |
| `mac_catalyst` | boolean | no | `false` | Builds the Mac Catalyst variant (not configurable) |
| `alwayslink` | boolean | no | `false` | Force-loads the framework's own static compilation archive into its dynamic link |
| `exported_deps` | list&lt;string&gt; | no | `[]` | Dependency target identifiers whose module interfaces flow to consumers |
| `bridging_header` | string | no |  | Objective-C bridging header used by Swift sources |
| `prefix_header` | string | no |  | Prefix header included before every C-family source |
| `prebuild_actions` | list&lt;string&gt; | no | `[]` | Ordered serialized source-generation actions. Records may opt into caching when they declare complete inputs and outputs; always-run records remain uncached (not configurable) |
| `enable_modules` | boolean | no | `false` | Emits and consumes a Clang module map for exported headers |
| `modulemap` | string | no |  | Authored Clang module map retained in the framework |
| `modulemap_headers` | list&lt;string&gt; | no | `[]` | Headers named by the authored module map, including private explicit submodules |
| `auxiliary_modulemaps` | list&lt;string&gt; | no | `[]` | Additional Clang module maps referenced by the framework's public Swift interface |

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `apple_linkable`, `apple_resource`, `apple_swift_plugin`, `native_linkable` | Libraries, resources, native linkables, and Swift compiler plugins linked or embedded by the framework |

## Providers

The target emits `apple_linkable`, `apple_framework`, and
`apple_bundle`.

Swift modules use the architecture and platform-qualified framework layout
produced by Xcode. Downstream Swift compilation discovers the binary module
through the framework search path.

Static framework imports keep their framework search path and module metadata
for compilation, but their binary is linked once as a static archive. They are
not also passed as a named framework or placed in the runtime framework closure.

The provider separates link-time and runtime framework closures. A downstream
link action links this framework, while the final application or test bundle
receives every framework needed at runtime. The framework's own archive is
fully loaded. Dependency archives use normal demand loading unless their
provider marks them as always linked, then stop at the dynamic link boundary
and are not linked into the final binary again.

Each dynamic framework, application, and test bundle resolves its own static
dependency closure. Once de-duplicates archive paths inside one link action,
but does not prune an archive merely because another dynamic framework also
contains it. That other framework does not necessarily re-export the archive's
Swift symbols.

| Field | Type | Meaning |
| --- | --- | --- |
| `framework_path` | string | Built framework directory |
| `framework_module_name` | string | Module name used by direct consumers |
| `framework_files` | list&lt;string&gt; | Framework outputs tracked by the action graph |
| `transitive_swiftmodule_dirs` | list&lt;string&gt; | Dependency Swift module search directories required by consumers; the framework's own module is found through its framework search path |
| `transitive_swiftmodule_inputs` | list&lt;string&gt; | Exact module artifacts available to downstream compiler actions |
| `transitive_exported_header_dirs` | list&lt;string&gt; | Dependency header search directories required to import this framework's module |
| `transitive_modulemaps` | list&lt;string&gt; | Dependency Clang module maps required to import this framework's module |
| `transitive_hmaps` | list&lt;string&gt; | Dependency header maps required to import this framework's module |
| `transitive_framework_search_dirs` | list&lt;string&gt; | Additional framework search directories required by consumers |
| `transitive_framework_files` | list&lt;string&gt; | Framework metadata inputs required by consumer compile actions |
| `transitive_vfs_overlays` | list&lt;string&gt; | Virtual file-system overlays required by consumer compile actions |
| `transitive_archives` | list&lt;string&gt; | Empty after the dynamic link boundary |
| `absorbed_static_archives` | list&lt;string&gt; | Static archives already linked into this framework |
| `transitive_plugin_dylibs` | list&lt;string&gt; | Host-loaded Swift compiler plugins required by downstream source |
| `transitive_plugin_executables` | list&lt;string&gt; | Host executable and declaring-module descriptors required by downstream source |
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
| Swift module | `.once/out/<target>/<product_name>.framework/Modules/<module_name>.swiftmodule/<target-triple>.swiftmodule` |
| Swift documentation | `.once/out/<target>/<product_name>.framework/Modules/<module_name>.swiftmodule/<target-triple>.swiftdoc` |
| Module map | `.once/out/<target>/<product_name>.framework/Modules/module.modulemap` |
| Property list | `.once/out/<target>/<product_name>.framework/Info.plist` |
| Code signature | `.once/out/<target>/<product_name>.framework/_CodeSignature/CodeResources` |

An application or test only needs to depend on the framework it uses directly.
Once carries nested dynamic framework dependencies to the final bundle,
de-duplicates them by framework path, embeds each bundle once, and signs the
result.

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
