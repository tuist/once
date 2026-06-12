# `apple_test_bundle`

XCTest bundle.

::: warning Schema only
The attribute and capability schema is wired up so manifests load
and [`once query schema apple_test_bundle`](/reference/cli/query)
inspects the contract, but no build actions run yet. Implementation
is pending.
:::

## Description

Builds and runs XCTest-style test bundles, including app-hosted
tests when a `test_host` is declared.

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
| `deps` | `apple_linkable`, `apple_application` | Code under test and optional host application |

## Providers

The target will emit `apple_test_bundle` and `apple_bundle` once the
implementation lands.

## Capabilities

| Capability | Output groups | Requires |
| --- | --- | --- |
| `build` | `default`, `bundle`, `dsyms` |  |
| `test` | `default`, `test_results`, `coverage` | `bundle` |
