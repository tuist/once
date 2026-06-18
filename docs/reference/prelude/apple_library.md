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
| `bridging_header` | string | no |  | ObjC bridging header that lets Swift sources see ObjC symbols |
| `swift_flags` | list&lt;string&gt; | no | `[]` | Extra Swift compiler flags |
| `clang_flags` | list&lt;string&gt; | no | `[]` | Extra Clang compiler flags |
| `defines` | list&lt;string&gt; | no | `[]` | `-D` preprocessor / Swift conditional compilation flags, propagated transitively |
| `enable_testing` | bool | no | `false` | Compile Swift with testability enabled for dependent tests |
| `library_evolution` | bool | no | `false` | Emit stable Swift module interfaces for binary compatibility |
| `enable_modules` | bool | no | `false` | Emit a `module.modulemap` and `.hmap` from `exported_headers` and pass `-fmodules` to Clang |
| `emit_dsym` | bool | no | `false` | Emit DWARF debug info so downstream target kinds can extract a `.dSYM` bundle |
| `sdk_frameworks` | list&lt;string&gt; | no | `[]` | Apple SDK frameworks linked by name, propagated transitively |
| `weak_sdk_frameworks` | list&lt;string&gt; | no | `[]` | Apple SDK frameworks linked weakly, propagated transitively |
| `sdk_dylibs` | list&lt;string&gt; | no | `[]` | Apple SDK dynamic libraries linked by name, propagated transitively |
| `linkopts` | list&lt;string&gt; | no | `[]` | Extra linker flags, propagated transitively |
| `alwayslink` | bool | no | `false` | Hint to downstream linker target kinds to force-load this archive (`-Wl,-force_load`) |
| `exported_deps` | list&lt;string&gt; | no | `[]` | Target ids from `deps` whose module interface flows through to consumers' compile path |

## Dep edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `apple_linkable`, `apple_resource`, `apple_swift_plugin` | Libraries, frameworks, resources, or Swift compiler plugins consumed by this library |

A dep that exposes a `plugin_dylib` provider field (see
[`swift_macro`](/reference/prelude/swift_macro)) is auto-detected and
threaded into the Swift compile as `-load-plugin-library <dylib>`.

## Providers

The target emits `apple_linkable` and `apple_module`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `default`, `binary`, `swiftmodule`, `generated_sources` |

## Compile pipeline

Each source extension routes to a different driver:

- **Swift** sources go through `xcrun --sdk <sdk> swiftc -emit-library -static -emit-module`. A `bridging_header` plumbs in via `-import-objc-header` so Swift can see ObjC symbols.
- **ObjC, C, and C++** sources each become an independent `xcrun --sdk <sdk> clang -c` action that writes one `.o` per source. The clang invocation pulls the SDK sysroot from `xcrun --show-sdk-path`, targets the active triple, and enables ARC for ObjC.
- **Mixed-language libraries** combine the Swift-only archive and the per-source clang objects with `xcrun libtool -static`. Swift-only or clang-only libraries skip the merge.
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
path, version banner, and any `DEVELOPER_DIR` override), the source
content digests, and each dep's action digest. A swap of Xcode, a
source edit, or a transitive dep change invalidates exactly the
affected cache slots.

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
| `transitive_exported_headers` | list&lt;string&gt; | Header paths from this and exported deps |
| `transitive_exported_header_dirs` | list&lt;string&gt; | Header search dirs from this and exported deps |
| `transitive_modulemaps` | list&lt;string&gt; | Modulemap paths to feed downstream consumers |
| `transitive_hmaps` | list&lt;string&gt; | Header-map paths to feed downstream consumers |
| `transitive_archives` | list&lt;string&gt; | Archive paths for the link line |
| `transitive_alwayslink_archives` | list&lt;string&gt; | Subset of archives that should be force-loaded |
| `transitive_sdk_frameworks` | list&lt;string&gt; | SDK frameworks to link |
| `transitive_weak_sdk_frameworks` | list&lt;string&gt; | Weakly linked SDK frameworks |
| `transitive_sdk_dylibs` | list&lt;string&gt; | SDK dynamic libraries to link |
| `transitive_linkopts` | list&lt;string&gt; | Extra linker flags |
| `transitive_defines` | list&lt;string&gt; | Preprocessor / conditional compilation flags |

The shape mirrors `SwiftInfo` and `CcInfo` from Bazel's Apple build model so
existing build engineers have a familiar mental model.

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

See the guide page on
[Configurable attributes](/guide/graph/apple#configurable-attributes)
for the overview.

## Outputs

| Output | Location |
| --- | --- |
| Static archive | `.once/out/<target>/<module_name>.a` |
| Swift module | `.once/out/<target>/<module_name>.swiftmodule` (single arch) or `.swiftmodule/<arch>.swiftmodule` (universal) |
| Swift doc | `.once/out/<target>/<module_name>.swiftdoc` or `.swiftmodule/<arch>.swiftdoc` |
| ObjC interop header | `.once/out/<target>/<module_name>-Swift.h` |
| Modulemap | `.once/out/<target>/module.modulemap` (when `enable_modules = true`) |
| Header map | `.once/out/<target>/<module_name>.hmap` (when `enable_modules = true`) |
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
