# `apple_xcframework_import`

Prebuilt XCFramework import.

## Description

Selects one platform and architecture slice from a prebuilt `.xcframework` and
exposes it to Apple consumers as a framework or static-library dependency. The bundle's
`Info.plist` is read to find the slice whose supported platform, platform
variant, and architecture match the requested ones. The selected framework's
linkage is detected from its binary, so a static slice is linked into the
consumer while a dynamic slice is linked and embedded.

The bundle is consumed where it sits. Nothing is recompiled. A static-library
slice exports its headers and module map to downstream compilation, then links
its archive into the final consumer. The module name defaults to the selected
framework name, or to the static-library module map when one is present.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `bundle` | string | yes |  | Workspace-relative `.xcframework` bundle |
| `platform` | string | yes |  | Apple platform whose slice is imported |
| `sdk_variant` | string | no | `simulator` | `simulator` or `device` slice selection; ignored on macOS |
| `arch` | string | no | host architecture | Architecture the slice must support |
| `module_name` | string | no | selected framework name | Framework module name |

`platform`, `sdk_variant`, `arch`, and `module_name` are not configurable by
platform select.

## Providers

The target emits `apple_linkable`, `apple_framework`, and `apple_bundle`, so
applications, frameworks, libraries, and test bundles can depend on it through
ordinary `deps` entries.

The optional `deps` edge accepts one `artifact` provider. It lets a generated
`archive_download` dependency materialize a remote bundle before this target is
analysed.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `default`, `framework` |

## Limitations

Resolution fails with a focused error when the bundle has no `Info.plist`, when
no slice matches the requested platform, variant, and architecture, or when the
selected slice is missing from the bundle. One target imports one slice, so
building for both a simulator and a device means selecting the variant through
the target's attributes.

## Example

```toml
[[target]]
name = "Example"
kind = "apple_xcframework_import"
srcs = ["Example.xcframework/Info.plist"]

[target.attrs]
bundle = "Example.xcframework"
platform = "macos"
arch = "arm64"
```
