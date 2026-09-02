# `apple_library`

Swift, Objective-C, C, and C++ static library.

## Description

Routes each source file through the driver that matches its
extension and emits a `.a` archive together with the Swift module
triple, ObjC interop header, and (optionally) a clang modulemap and
binary header map. Multi-arch targets fan out per-arch compiles and
merge them with `lipo`.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `platform` | string | yes |  | Apple platform such as `ios`, `macos`, `tvos`, `watchos`, or `visionos` |
| `minimum_os` | string | no |  | Minimum supported OS version (deployment target) |
| `target_sdk_version` | string | no | `minimum_os` | Build-time SDK version baked into the triple |
| `sdk_variant` | string | no | `"simulator"` | `simulator` or `device`. Ignored on macOS (always `macosx`) |
| `archs` | list&lt;string&gt; | no | `[]` | Target architectures (`arm64`, `x86_64`, `arm64e`, `arm64_32`). Empty defaults to the host arch; multi-arch fans out per-arch compiles and combines them with `lipo` |
| `mac_catalyst` | bool | no | `false` | Build the iOSMac (Mac Catalyst) variant. Requires `platform = macos`; rewrites the triple to `<arch>-apple-ios<minOS>-macabi` |
| `module_name` | string | no | target name | Compiled module name (not configurable) |
| `xcode_developer_dir` | string | no |  | Pin a specific Xcode by overriding `DEVELOPER_DIR`. Folded into the action cache key |
| `headers` | list&lt;string&gt; | no | `[]` | Public or private C-family headers compiled with this target |
| `exported_headers` | list&lt;string&gt; | no | `[]` | Headers made available to dependent targets |
| `exported_header_dirs` | list&lt;string&gt; | no | `[]` | Header search directories made available to dependent targets |
| `private_header_dirs` | list&lt;string&gt; | no | `[]` | Header search directories used only while compiling this target |
| `resources` | list&lt;string&gt; | no | `[]` | Files and directory roots placed in this library's propagated resource bundle |
| `structured_resources` | list&lt;string&gt; | no | `[]` | Resource directory roots whose own basename is preserved inside the propagated bundle |
| `resource_bundle_name` | string | no |  | Name of the propagated resource bundle. The `.bundle` suffix is added when omitted |
| `resource_bundle_id` | string | no |  | Bundle identifier written to the propagated resource bundle metadata |
| `bridging_header` | string | no |  | ObjC bridging header that lets Swift sources see ObjC symbols |
| `prefix_header` | string | no |  | Prefix header included before every C-family source |
| `swift_flags` | list&lt;string&gt; | no | `[]` | Extra Swift compiler flags |
| `clang_flags` | list&lt;string&gt; | no | `[]` | Extra Clang compiler flags |
| `per_source_clang_flags` | map&lt;string, string&gt; | no | `{}` | Clang flag lists encoded as [JavaScript Object Notation (JSON)](https://www.json.org/json-en.html) and keyed by source path. Xcode adapters use this to retain per-file compiler settings |
| `defines` | list&lt;string&gt; | no | `[]` | Compatibility conditions passed to both Swift and Clang |
| `swift_defines` | list&lt;string&gt; | no | `[]` | Swift conditional compilation conditions |
| `clang_defines` | list&lt;string&gt; | no | `[]` | C-family preprocessor definitions |
| `enable_testing` | bool | no | `false` | Compile Swift with testability enabled for dependent tests |
| `swift_testing` | bool | no | `false` | Compile sources that import the Swift Testing framework |
| `xctest_support` | bool | no | `false` | Compile sources that import the XCTest framework |
| `library_evolution` | bool | no | `false` | Emit stable Swift module interfaces for binary compatibility |
| `enable_modules` | bool | no | `false` | Emit a `module.modulemap` and `.hmap` from `exported_headers` and pass `-fmodules` to Clang |
| `emit_dsym` | bool | no | `false` | Emit DWARF debug info so downstream target kinds can extract a `.dSYM` bundle |
| `sdk_frameworks` | list&lt;string&gt; | no | `[]` | Apple SDK frameworks linked by name, propagated transitively |
| `weak_sdk_frameworks` | list&lt;string&gt; | no | `[]` | Apple SDK frameworks linked weakly, propagated transitively |
| `sdk_dylibs` | list&lt;string&gt; | no | `[]` | Apple SDK dynamic libraries linked by name, propagated transitively |
| `linkopts` | list&lt;string&gt; | no | `[]` | Extra linker flags, propagated transitively |
| `alwayslink` | bool | no | `false` | Hint to downstream linker target kinds to force-load this archive (`-Wl,-force_load`) |
| `exported_deps` | list&lt;string&gt; | no | `[]` | Target ids from `deps` whose module interface flows through to consumers' compile path |
| `prebuild_actions` | list&lt;string&gt; | no | `[]` | Adapter-owned serialized build preparation actions that run before compilation. Records may opt into caching when they declare complete inputs and outputs; always-run records remain uncached |
| `modulemap` | string | no |  | Authored Clang module map retained instead of generating one |
| `modulemap_headers` | list&lt;string&gt; | no | `[]` | Headers named by the authored module map, including private explicit submodules |
| `auxiliary_modulemaps` | list&lt;string&gt; | no | `[]` | Additional Clang module maps referenced by this module's public Swift interface |

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `apple_linkable`, `apple_resource`, `apple_swift_plugin`, `native_linkable` | Libraries, frameworks, resources, native linkables, or Swift compiler plugins consumed by this library |

A dependency that exposes a compiler-plugin executable (see
[`swift_macro`](/reference/prelude/swift_macro)) is auto-detected and loaded
with its declaring module. The resulting `transitive_plugin_executables`
field keeps the host tool available to downstream targets that use a macro
declared by the library. Library-style compiler plugins remain supported
through `transitive_plugin_dylibs`.

When `resource_bundle_name` is set, the target creates a resource bundle and
propagates it through static library dependencies to the final application.
Directory roots are merged at the bundle root unless listed in
`structured_resources`. Localized directories retain their structure,
interface files are compiled, and managed object models are
compiled into runtime model bundles. The final application embeds and signs
each propagated resource bundle before signing its outer bundle.

## Providers

The target emits `apple_linkable` and `apple_module`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `default`, `binary`, `swiftmodule`, `generated_sources` |

## Compile pipeline

Each source extension routes to a different driver:

- **Swift** sources use one compiler action to emit the module, compatibility header, and object files. A separate archive action packages the objects. A `bridging_header` plumbs in via `-import-objc-header` so Swift can see Objective-C symbols.
- **ObjC, C, and C++** sources each become an independent `xcrun --sdk <sdk> clang -c` action that writes one `.o` per source. The clang invocation pulls the SDK sysroot from `xcrun --show-sdk-path`, targets the active triple, and enables ARC for ObjC.
- **Mixed-language libraries** combine the Swift archive and per-source Clang objects with the platform archive tool. Swift-only libraries use their Swift archive directly, while Clang-only libraries archive their objects once.
- **Multi-arch** targets repeat the swift + clang + libtool chain per architecture, then run `xcrun lipo -create` on the per-arch archives to produce the final universal archive. Single-arch targets skip lipo entirely.

Dep `swiftmodule` directories are forwarded as `-I` search paths so
`import` statements resolve. With `enable_modules = true` the impl
writes a `module.modulemap` from `exported_headers`, threads it into
consumers through the provider, and also writes a binary header map
(`<module_name>.hmap`) mapping each exported header's basename and
`<module_name>/<basename>` form to its workspace-relative path. The
hmap is passed to clang and swiftc via `-I`, covering the
`#include "Foo.h"` and `#include <Module/Foo.h>` lookup styles a
modulemap alone does not.

The action cache key composes the resolved toolchain identity (each
of swiftc, clang, libtool, and lipo carries its own `xcrun`-resolved
path, version banner, and any `DEVELOPER_DIR` override), source
content, and declared generated inputs. Imported Swift modules are
fingerprinted by their artifact content. A private implementation edit
that preserves a module can therefore reuse downstream compilation,
while the changed archive still participates in later link actions.

## Provider record

`apple_library` returns a record consumers read through
`ctx["deps"]`. Fields:

| Field | Type | Meaning |
| --- | --- | --- |
| `label_id` | string | Canonical target id |
| `swiftmodule_dir` | string | Directory holding the `.swiftmodule`, added to `-I` by consumers |
| `archive` | string | Final static archive path |
| `objc_header` | string | Generated `-Swift.h` ObjC interop header |
| `modulemap` | string | Path to the emitted `module.modulemap`, or empty |
| `hmap` | string | Path to the emitted `.hmap`, or empty |
| `exported_headers` | list&lt;string&gt; | Headers this target re-exposes to consumers |
| `exported_header_dirs` | list&lt;string&gt; | Parent directories of the exported headers, added to `-I` by consumers |
| `alwayslink` | bool | Hint propagated for force-load |
| `transitive_swiftmodule_dirs` | list&lt;string&gt; | Module search paths (gated by `exported_deps`) |
| `transitive_swiftmodule_inputs` | list&lt;string&gt; | Exact architecture-matching module artifacts consumed by downstream compiler actions, propagated through `exported_deps` |
| `transitive_exported_headers` | list&lt;string&gt; | Header paths from this and exported deps |
| `transitive_generated_headers` | list&lt;string&gt; | Generated compatibility headers required by downstream compile actions |
| `transitive_exported_header_dirs` | list&lt;string&gt; | Header search dirs from this and exported deps |
| `transitive_modulemaps` | list&lt;string&gt; | Modulemap paths to feed downstream consumers |
| `transitive_hmaps` | list&lt;string&gt; | Header-map paths to feed downstream consumers |
| `transitive_framework_search_dirs` | list&lt;string&gt; | Framework search paths required by downstream compile actions |
| `transitive_framework_files` | list&lt;string&gt; | Complete generated framework file sets required as action inputs |
| `transitive_vfs_overlays` | list&lt;string&gt; | [Virtual file system overlay](https://clang.llvm.org/docs/LibTooling.html#virtual-files) manifests that preserve source-header identity through generated framework paths |
| `transitive_archives` | list&lt;string&gt; | Archive paths for the link line |
| `transitive_alwayslink_archives` | list&lt;string&gt; | Subset of archives that should be force-loaded |
| `transitive_sdk_frameworks` | list&lt;string&gt; | SDK frameworks to link |
| `transitive_weak_sdk_frameworks` | list&lt;string&gt; | Weakly linked SDK frameworks |
| `transitive_sdk_dylibs` | list&lt;string&gt; | SDK dynamic libraries to link |
| `transitive_linkopts` | list&lt;string&gt; | Extra linker flags |
| `transitive_plugin_dylibs` | list&lt;string&gt; | Host-loaded Swift compiler plugins required by downstream source |
| `transitive_plugin_executables` | list&lt;string&gt; | Host executable and declaring-module descriptors required by downstream source |
| `transitive_defines` | list&lt;string&gt; | Preprocessor / conditional compilation flags |
| `transitive_link_framework_bundles` | list&lt;record&gt; | Dynamic framework bundles carried to the next link action |
| `transitive_framework_bundles` | list&lt;record&gt; | Dynamic framework bundles carried to the final application or test bundle |
| `transitive_frameworks` | list&lt;string&gt; | Compatibility view of runtime framework paths |
| `transitive_resource_bundles` | list&lt;record&gt; | Resource bundle paths and complete file sets carried to the final application |

Downstream Apple targets use this record to collect the complete compile and
link context without inspecting the dependency's target kind. Dynamic
framework dependencies continue through static libraries automatically, so a
final application or test does not repeat dependencies that it never imports.

## Configurable attributes

Every attribute except `module_name`, `archs`, `platform`,
`sdk_variant`, and `mac_catalyst` accepts a `select` value.
Configuration tokens for matching come from the target's resolved
literal values:

| Token group | Source | Example values |
| --- | --- | --- |
| Platform | `platform` | `ios`, `macos`, `tvos`, `watchos`, `visionos` |
| SDK variant | `sdk_variant` | `simulator`, `device` |
| Architecture | each entry of `archs` | `arm64`, `x86_64`, `arm64e`, `arm64_32` |
| Mac Catalyst | literal token when `mac_catalyst = true` | `mac_catalyst` |

Branch keys can combine tokens with `:` (e.g. `ios:simulator`); when
several branches match the longest matching key wins. A `default`
branch is selected when no other branch matches.

```toml
[target.attrs]
sdk_frameworks = { select = { ios = ["UIKit"], macos = ["AppKit"] } }
```

See [Choose Values by Configuration](/guide/graph/apple#choose-values-by-configuration)
for the guided overview.

## Outputs

| Output | Location |
| --- | --- |
| Static archive | `.once/out/<target>/<module_name>.a` |
| Swift module | `.once/out/<target>/<module_name>.swiftmodule` (single arch) or `.swiftmodule/<arch>.swiftmodule` (universal) |
| Swift doc | `.once/out/<target>/<module_name>.swiftdoc` or `.swiftmodule/<arch>.swiftdoc` |
| ObjC interop header | `.once/out/<target>/<module_name>-Swift.h` |
| Modulemap | `.once/out/<target>/module.modulemap` (when `enable_modules = true`) |
| Header map | `.once/out/<target>/<module_name>.hmap` (when `enable_modules = true`) |
| Resource bundle | `.once/out/<target>/<resource_bundle_name>.bundle` (when `resource_bundle_name` is set) |
| Per-source clang objects | `.once/out/<target>/<sanitised_source>[-<arch>].o` |

## Example

```toml
[[target]]
name = "AppCore"
kind = "apple_library"
srcs = ["Sources/**/*.swift", "Sources/**/*.m"]
deps = ["./StringifyMacro"]

[target.attrs]
platform = "ios"
minimum_os = "17.0"
archs = ["arm64", "x86_64"]
sdk_frameworks = ["UIKit"]
enable_modules = true
exported_headers = ["include/AppCore.h"]
```
