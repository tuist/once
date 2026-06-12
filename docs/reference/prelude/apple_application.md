# `apple_application`

Apple application bundle.

::: warning Schema only
The attribute and capability schema is wired up so manifests load
and [`once query schema apple_application`](/reference/cli/query)
inspects the contract, but no build actions run yet. Implementation
is pending.
:::

## Description

Builds an Apple application bundle with resources, Info.plist
metadata, and signing inputs.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `platform` | string | yes |  | Apple platform for the application |
| `bundle_id` | string | yes |  | Application bundle identifier |
| `minimum_os` | string | no |  | Minimum supported OS version |
| `families` | list&lt;string&gt; | no | `[]` | Supported device families (`iphone`, `ipad`) |
| `product_name` | string | no | target name | Application product name (not configurable) |
| `resources` | list&lt;string&gt; | no | `[]` | Resource and asset catalog glob patterns |
| `asset_catalogs` | list&lt;string&gt; | no | `[]` | Asset catalog paths compiled into the application bundle |
| `info_plist` | string | no |  | Info.plist template path |
| `info_plist_substitutions` | map&lt;string,string&gt; | no | `{}` | Values substituted into the generated Info.plist |
| `entitlements` | string | no |  | Entitlements plist path |
| `provisioning_profile` | string | no |  | Provisioning profile label or path used for signing |
| `signing` | string | no | `ad_hoc` | Signing mode or policy name |
| `sdk_frameworks` | list&lt;string&gt; | no | `[]` | Apple SDK frameworks linked by name |
| `weak_sdk_frameworks` | list&lt;string&gt; | no | `[]` | Apple SDK frameworks linked weakly |
| `sdk_dylibs` | list&lt;string&gt; | no | `[]` | Apple SDK dynamic libraries linked by name |

## Dep edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `apple_linkable`, `apple_framework`, `apple_resource` | Libraries, frameworks, and resources embedded in the app |

## Providers

The target will emit `apple_application` and `apple_bundle` once the
implementation lands.

## Capabilities

| Capability | Output groups | Requires |
| --- | --- | --- |
| `build` | `default`, `bundle`, `dsyms` |  |
| `run` | `default` | `bundle` |
