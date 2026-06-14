# `apple_test_bundle`

XCTest bundle.

## Description

Builds XCTest-style test bundles for an external runner.

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

## Dep edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `apple_linkable`, `apple_application`, `apple_swift_plugin` | Code under test, optional host application, and Swift compiler plugins |

## Providers

The target emits `apple_test_bundle` and `apple_bundle`.

## Capabilities

| Capability | Output groups | Requires |
| --- | --- | --- |
| `build` | `default`, `bundle`, `dsyms` |  |

## Limitations

App-hosted tests, resource bundling, asset catalogs, custom Info.plist
templates, entitlements, destinations, test plans, and test runner
environment variables are declared in the schema for graph
compatibility but are not implemented yet. Non-empty values for those
attrs fail analysis instead of being ignored.
