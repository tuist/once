# `apple_resource_bundle`

Apple resource bundle.

## Description

Processes resources into a named `.bundle` directory and propagates the bundle
through Apple dependency edges to the final application. Localized resources,
interface files, asset catalogs, and managed object models use the same
resource processing pipeline as application and framework resources.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `platform` | string | yes |  | Apple platform such as `ios`, `macos`, `tvos`, `watchos`, or `visionos` |
| `minimum_os` | string | no | `13.0` | Minimum supported operating system version |
| `sdk_variant` | string | no | `simulator` | `simulator` or `device`; ignored on macOS |
| `bundle_name` | string | no | target name | Bundle name. The `.bundle` suffix is added when omitted |
| `bundle_id` | string | no | `dev.once.<bundle_name>.resources` | Bundle identifier written to generated metadata |
| `resources` | list&lt;string&gt; | no | `[]` | Files and directory roots processed into the bundle |
| `structured_resources` | list&lt;string&gt; | no | `[]` | Directory roots whose own basename is preserved inside the bundle |
| `xcode_developer_dir` | string | no | active Xcode | Xcode developer directory used to resolve resource tools |

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `apple_resource` | Resource bundles propagated alongside this bundle |

## Providers

The target emits `apple_resource`. Its provider contains a deterministic list
of bundle paths and files that downstream applications embed and sign.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `default`, `bundle` |

## Outputs

| Output | Location |
| --- | --- |
| Resource bundle | `.once/out/<target>/<bundle_name>.bundle` |
| Property list | `.once/out/<target>/<bundle_name>.bundle/Info.plist` |

## Example

```toml
[[target]]
name = "AppResources"
kind = "apple_resource_bundle"

[target.attrs]
platform = "ios"
resources = ["Resources/**"]
structured_resources = ["Resources/Fixtures"]
```
