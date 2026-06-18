# `apple_application`

Apple application bundle.

## Description

Builds an Apple application bundle with a generated Info.plist,
linked dependencies, embedded frameworks, and ad-hoc signing. The
`run` capability builds the required bundle and then executes a
launch action declared by the target kind that bypasses the action cache.

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
| `signing_identity` | string | no |  | Local signing identity selector used for development device signing |
| `signing` | string | no | `ad_hoc` | Signing mode or policy name |
| `sdk_frameworks` | list&lt;string&gt; | no | `[]` | Apple SDK frameworks linked by name |
| `weak_sdk_frameworks` | list&lt;string&gt; | no | `[]` | Apple SDK frameworks linked weakly |
| `sdk_dylibs` | list&lt;string&gt; | no | `[]` | Apple SDK dynamic libraries linked by name |

## Dep edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `apple_linkable`, `apple_framework`, `apple_resource`, `apple_swift_plugin` | Libraries, frameworks, resources, and Swift compiler plugins embedded in the app |

## Providers

The target emits `apple_application` and `apple_bundle`.

## Capabilities

| Capability | Output groups | Requires |
| --- | --- | --- |
| `build` | `default`, `bundle`, `dsyms` |  |
| `run` | `default` | `bundle` |

## Running

`once run` launches macOS apps with the host app launcher and iOS
simulator apps with `simctl` boot, install, and launch. The launch
action writes a run record under the target's output directory and is
marked uncacheable by the target kind, so repeated runs execute the launch
again instead of replaying an action-cache hit. Device launch support
is not implemented yet.

## Limitations

Resource bundling, asset catalogs, custom Info.plist templates,
entitlements, provisioning profiles, signing identities, and non-ad-hoc signing are
declared in the schema for graph compatibility but are not implemented
yet. Device launch support is also not implemented yet. Non-empty
values for unsupported attrs fail analysis instead of being ignored.
