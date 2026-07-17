# `apple_test_bundle`

Apple test bundle.

## Description

Builds Apple test targets and runs XCTest or Swift Testing tests through the
generic Once test capability. Attributes whose names contain `sdk` configure
the
[Apple software development kit (SDK)](https://developer.apple.com/documentation/xcode)
used for the build.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `platform` | string | yes |  | Apple platform for the tests |
| `minimum_os` | string | no | `13.0` | Minimum supported operating system version |
| `target_sdk_version` | string | no | `minimum_os` | Software development kit version used in the target triple |
| `sdk_variant` | string | no | `simulator` | `simulator` or `device`; ignored on macOS (not configurable) |
| `xcode_developer_dir` | string | no | active Xcode | Xcode developer directory used to resolve build tools |
| `product_name` | string | no | target name | Test bundle product name (not configurable) |
| `test_host` | target | no |  | Application target hosting the test bundle |
| `resources` | list&lt;string&gt; | no | `[]` | Resource glob patterns bundled into the test bundle |
| `asset_catalogs` | list&lt;string&gt; | no | `[]` | Asset catalog paths compiled into the test bundle |
| `info_plist` | string | no |  | Info.plist template path |
| `entitlements` | string | no |  | Entitlements plist path |
| `destination` | string | no |  | Simulator, device, or local destination selector |
| `test_plan` | string | no |  | XCTest plan path |
| `test_env` | map&lt;string,string&gt; | no | `{}` | Environment variables passed to the test runner |
| `swift_flags` | list&lt;string&gt; | no | `[]` | Extra Swift compiler flags |
| `swift_testing` | bool | no | `false` | Run sources that use Swift Testing (`import Testing`) through the generic Once test capability |
| `labels` | list&lt;string&gt; | no | `[]` | Agent-readable labels used for filtering or policy |

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `apple_linkable`, `apple_application`, `apple_swift_plugin`, `native_linkable` | Code under test, optional host application, native linkables, and Swift compiler plugins |

## Providers

The target emits `apple_test_bundle`, `apple_bundle`, and `once_test_info`.

## Capabilities

| Capability | Output groups | Requires |
| --- | --- | --- |
| `build` | `default`, `bundle`, `dsyms` |  |
| `test` | `default`, `test_results`, `coverage` |  |

## Outputs

| Output | Location |
| --- | --- |
| Test bundle | `.once/out/<target>/<product_name>.xctest` |
| macOS test binary | `.once/out/<target>/<product_name>.xctest/Contents/MacOS/<product_name>` |
| Other Apple platform test binary | `.once/out/<target>/<product_name>.xctest/<product_name>` |
| macOS property list | `.once/out/<target>/<product_name>.xctest/Contents/Info.plist` |
| Other Apple platform property list | `.once/out/<target>/<product_name>.xctest/Info.plist` |
| macOS code signature | `.once/out/<target>/<product_name>.xctest/Contents/_CodeSignature/CodeResources` |
| Other Apple platform code signature | `.once/out/<target>/<product_name>.xctest/_CodeSignature/CodeResources` |
| Test results | `.once/out/<target>/test/test_results.json` after `once test` |
| Test log | `.once/out/<target>/test/swift-testing.log` for Swift Testing or `xctest.log` for XCTest |
| Native runner output | `.once/out/<target>/test/native_results.txt` |

## Limitations

App-hosted tests, resource bundling, asset catalogs, custom property-list
templates, entitlements, destinations, and test plans are unsupported. The
target rejects non-empty values for those attributes instead of ignoring them.
Test execution is limited to macOS logic tests and iOS simulator bundles.

## Example

```toml
[[target]]
name = "AppTests"
kind = "apple_test_bundle"
srcs = ["AppTests/Sources/*.swift"]

[target.attrs]
platform = "macos"
swift_testing = true
labels = ["swift-testing"]
```
