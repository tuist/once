# `swift_macro`

Swift compiler-plugin dylib built for the host.

## Description

Compiles Swift sources into a loadable plugin (`lib<module_name>.dylib`)
that the Swift compiler loads at compile time. The macro implementation
typically depends on a swift-syntax checkout the user provides via
`deps`. Any [`apple_library`](/reference/prelude/apple_library) dep
edge that points at a `swift_macro` target picks up
`-load-plugin-library <dylib>` automatically.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `minimum_os` | string | no | `"13.0"` | Minimum macOS version for the host plugin |
| `module_name` | string | no | target name | Compiled module name (not configurable) |
| `swift_flags` | list&lt;string&gt; | no | `[]` | Extra Swift compiler flags |
| `xcode_developer_dir` | string | no |  | Pin a specific Xcode by overriding `DEVELOPER_DIR`. Folded into the action cache key |

## Dep edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `apple_linkable` | Libraries the plugin links against (typically a swift-syntax checkout) |

## Providers

The target emits `apple_swift_plugin`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `default`, `plugin_dylib`, `swiftmodule` |

## Provider record

| Field | Type | Meaning |
| --- | --- | --- |
| `label_id` | string | Canonical target id |
| `plugin_dylib` | string | Path to the produced `lib<module_name>.dylib` |
| `plugin_module_name` | string | Module name a downstream `apple_library` passes to `-load-plugin-library` |

## Outputs

| Output | Location |
| --- | --- |
| Plugin dylib | `.once/out/<target>/lib<module_name>.dylib` |
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

## Limitations

The target kind does not vendor swift-syntax. The user supplies it as a
regular dep edge, which matches how established Apple build systems handle
the same dependency. Until a swift-syntax checkout is available as
an `apple_library`, `swift_macro` targets cannot build end to end.
