# `apple_framework`

Apple framework bundle.

::: warning Schema only
The attribute and capability schema is wired up so manifests load
and [`once query schema apple_framework`](/reference/cli/query)
inspects the contract, but no build actions run yet. Implementation
is pending.
:::

## Description

Builds an Apple framework product with module metadata and optional
resources, asset catalogs, privacy manifests, headers, and debug
symbol outputs.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `platform` | string | yes |  | Apple platform for the framework |
| `minimum_os` | string | no |  | Minimum supported OS version |
| `bundle_id` | string | no |  | Framework bundle identifier |
| `product_name` | string | no | target name | Framework product name (not configurable) |
| `headers` | list&lt;string&gt; | no | `[]` | Headers packaged with the framework |
| `exported_headers` | list&lt;string&gt; | no | `[]` | Headers exported to downstream consumers |
| `resources` | list&lt;string&gt; | no | `[]` | Resource glob patterns bundled into the framework |
| `asset_catalogs` | list&lt;string&gt; | no | `[]` | Asset catalog paths compiled into the framework bundle |
| `privacy_manifest` | string | no |  | Privacy manifest placed in the framework bundle |
| `sdk_frameworks` | list&lt;string&gt; | no | `[]` | Apple SDK frameworks linked by name |
| `weak_sdk_frameworks` | list&lt;string&gt; | no | `[]` | Apple SDK frameworks linked weakly |

## Dep edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `apple_linkable`, `apple_resource` | Libraries and resources linked or embedded by the framework |

## Providers

The target will emit `apple_linkable`, `apple_framework`, and
`apple_bundle` once the implementation lands.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `default`, `framework`, `dsyms`, `swiftmodule` |
