# `swift_macro`

Swift compiler-plugin executable built for the host.

## Description

Compiles Swift sources into a macOS host executable that the Swift compiler
loads at compile time. The macro implementation typically depends on a
swift-syntax checkout supplied through `deps`. Any
[`apple_library`](/reference/prelude/apple_library) dependency edge that
reaches a `swift_macro` target picks up the executable and declaring module
automatically.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `minimum_os` | string | no | `"13.0"` | Minimum macOS version for the host plugin |
| `module_name` | string | no | target name | Compiled module name (not configurable) |
| `swift_flags` | list&lt;string&gt; | no | `[]` | Extra Swift compiler flags |
| `xcode_developer_dir` | string | no |  | Pin a specific Xcode by overriding `DEVELOPER_DIR`. Folded into the action cache key |

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `apple_linkable` | Libraries the plugin links against (typically a swift-syntax checkout) |

## Providers

The target emits `apple_swift_plugin`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `default`, `plugin_executable`, `swiftmodule` |

## Provider record

| Field | Type | Meaning |
| --- | --- | --- |
| `label_id` | string | Canonical target id |
| `plugin_executable` | string | Path to the produced macOS host executable |
| `plugin_module_name` | string | Declaring module name paired with the executable by downstream compilers |
| `transitive_plugin_executables` | list&lt;string&gt; | `<path>#<module>` descriptors propagated through libraries and frameworks |

## Outputs

| Output | Location |
| --- | --- |
| Plugin executable | `.once/out/<target>/<module_name>-tool` |
| Swift module | `.once/out/<target>/<module_name>.swiftmodule` |

## Example

```toml
[[target]]
name = "StringifyMacro"
kind = "swift_macro"
srcs = ["Sources/**/*.swift"]
deps = [
  "//third_party/swift-syntax:SwiftSyntax",
  "//third_party/swift-syntax:SwiftCompilerPlugin",
]
```
