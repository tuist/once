# `swift_macro`

Swift compiler-plugin executable built for the host.

## Description

Compiles Swift sources into a macOS host executable that the Swift compiler
loads at compile time. The macro implementation typically depends on a
swift-syntax checkout supplied through `deps`. Any
[`apple_library`](/reference/prelude/apple_library) dependency edge that
reaches a `swift_macro` target picks up the executable and declaring module
automatically.

A macro is a tool for the targets that expand it, so its code stays out of what
they link. A target that unit-tests the macro is the exception: it imports the
module to reach the implementation types, so an
[`apple_test_bundle`](/reference/prelude/apple_test_bundle) that depends on a
macro compiles and links against it. A macro only ever builds for the host, so
such a test target builds for the host too, and so does everything it depends
on.

## Attributes

`explicit_modules` is a boolean, defaulting to `false`, that enables
compiler-scanned, cacheable Swift and Clang module actions.
`dependency_check` accepts `"off"` (the default) or `"error"`; checking
requires explicit modules. See [explicit modules](/guide/graph/apple#explicit-modules-and-dependency-checks)
for native propagation, dependency errors, and current limitations.
The resolver-owned `_declared_deps` metadata preserves declarations before
import inference and should not be authored manually.

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
