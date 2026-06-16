# `apple_test_bundle`

Apple test bundle.

## Description

Builds Apple test targets and can run XCTest or Swift Testing tests
through the generic Once test capability. Swift Testing sources compile
into the same `.xctest` bundle shape as XCTest tests; Once does not
generate a Swift package wrapper for them.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `platform` | string | yes |  | Apple platform for the tests |
| `minimum_os` | string | no |  | Minimum supported OS version |
| `test_host` | target | no |  | Application target hosting the test bundle |
| `resources` | list&lt;string&gt; | no | `[]` | Resource glob patterns bundled into the test bundle |
| `asset_catalogs` | list&lt;string&gt; | no | `[]` | Asset catalog paths compiled into the test bundle |
| `info_plist` | string | no |  | Info.plist template path |
| `entitlements` | string | no |  | Entitlements plist path |
| `destination` | string | no |  | Simulator, device, or local destination selector |
| `test_plan` | string | no |  | XCTest plan path |
| `test_env` | map&lt;string,string&gt; | no | `{}` | Environment variables passed to the test runner |
| `swift_testing` | bool | no | `false` | Run sources that use Swift Testing (`import Testing`) through the generic Once test capability |
| `labels` | list&lt;string&gt; | no | `[]` | Agent-readable labels used for filtering or policy |

## Dep edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `apple_linkable`, `apple_application`, `apple_swift_plugin` | Code under test, optional host application, and Swift compiler plugins |

## Providers

The target emits `apple_test_bundle`, `apple_bundle`, and `once_test_info`.

## Capabilities

| Capability | Output groups | Requires |
| --- | --- | --- |
| `build` | `default`, `bundle`, `dsyms` |  |
| `test` | `default`, `test_results`, `coverage` |  |

## Limitations

App-hosted tests, resource bundling, asset catalogs, custom Info.plist
templates, entitlements, destinations, and test plans are declared in
the schema for graph compatibility but are not implemented yet.
Non-empty values for those attrs fail analysis instead of being
ignored. Test execution currently supports macOS logic tests and iOS
simulator bundles. Device runners still need xctestrun support.
