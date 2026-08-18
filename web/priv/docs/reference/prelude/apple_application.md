# `apple_application`

Apple application bundle.

## Description

Builds an Apple application bundle with a generated `Info.plist` property-list
file, linked dependencies, embedded frameworks, and ad-hoc signing. The `run`
capability builds the required bundle and launches it. Attributes whose names
contain `sdk` configure the
[Apple software development kit (SDK)](https://developer.apple.com/documentation/xcode)
used for the build.

Framework dependencies are transitive at runtime. Declare the framework the
application imports directly. Once links the direct dynamic boundary, embeds
its complete framework closure, removes duplicate paths, signs every embedded
framework, and then signs the application.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `platform` | string | yes |  | Apple platform for the application |
| `bundle_id` | string | yes |  | Application bundle identifier |
| `minimum_os` | string | no | `13.0` | Minimum supported operating system version |
| `target_sdk_version` | string | no | `minimum_os` | Software development kit version used in the target triple |
| `sdk_variant` | string | no | `simulator` | `simulator` or `device`; ignored on macOS (not configurable) |
| `xcode_developer_dir` | string | no | active Xcode | Xcode developer directory used to resolve build tools |
| `families` | list&lt;string&gt; | no | `[]` | Supported device families (`iphone`, `ipad`); an empty list uses `iphone` |
| `product_name` | string | no | target name | Application product name (not configurable) |
| `resources` | list&lt;string&gt; | no | `[]` | Resource files and directory roots placed in the application bundle |
| `structured_resources` | list&lt;string&gt; | no | `[]` | Resource directory roots whose own basename is preserved inside the application bundle |
| `asset_catalogs` | list&lt;string&gt; | no | `[]` | Asset catalog paths compiled into the application bundle |
| `info_plist` | string | no |  | Info.plist template path |
| `info_plist_substitutions` | map&lt;string,string&gt; | no | `{}` | Values substituted into the generated Info.plist |
| `entitlements` | string | no |  | Entitlements plist path |
| `entitlements_substitutions` | map&lt;string,string&gt; | no | `{}` | Build-setting values substituted into `$(NAME)` or `${NAME}` placeholders before signing |
| `development_team` | string | no |  | Apple development team identifier used to derive simulator application identity |
| `provisioning_profile` | string | no |  | Provisioning profile label or path used for signing |
| `signing_identity` | string | no |  | Local signing identity selector used for development device signing |
| `signing` | string | no | `ad_hoc` | Signing mode or policy name |
| `sdk_frameworks` | list&lt;string&gt; | no | `[]` | Apple software development kit frameworks linked by name |
| `weak_sdk_frameworks` | list&lt;string&gt; | no | `[]` | Apple software development kit frameworks linked weakly |
| `sdk_dylibs` | list&lt;string&gt; | no | `[]` | Apple software development kit dynamic libraries linked by name |
| `linkopts` | list&lt;string&gt; | no | `[]` | Extra linker flags |
| `swift_flags` | list&lt;string&gt; | no | `[]` | Extra Swift compiler flags |
| `clang_flags` | list&lt;string&gt; | no | `[]` | Extra Clang compiler flags for C, C++, Objective-C, and Objective-C++ sources |
| `per_source_clang_flags` | map&lt;string,string&gt; | no | `{}` | JSON-encoded Clang compiler flag lists keyed by source path |
| `defines` | list&lt;string&gt; | no | `[]` | Compatibility conditions passed to both Swift and Clang |
| `swift_defines` | list&lt;string&gt; | no | `[]` | Swift conditional compilation conditions |
| `clang_defines` | list&lt;string&gt; | no | `[]` | C-family preprocessor definitions |
| `exported_header_dirs` | list&lt;string&gt; | no | `[]` | Header search directories exported by the application target |
| `private_header_dirs` | list&lt;string&gt; | no | `[]` | Private header search directories used while compiling the application |
| `bridging_header` | string | no |  | Objective-C bridging header imported into Swift sources |
| `prefix_header` | string | no |  | Prefix header included before every C-family source |
| `prebuild_actions` | list&lt;string&gt; | no | `[]` | Adapter-owned serialized build preparation actions that run before compilation. Records may opt into caching when they declare complete inputs and outputs; always-run records remain uncached |
| `application_extension` | bool | no | `false` | Build as an app extension: entered through `NSExtensionMain` and compiled against the app-extension-safe interface |
| `app_icon` | string | no |  | Asset catalog app-icon set name compiled into the application icon |
| `enable_testing` | bool | no | `false` | Compile Swift with testability enabled so hosted test bundles can use `@testable import` |

For simulator applications, processed platform entitlements are embedded in the executable's `__TEXT,__entitlements` and `__TEXT,__ents_der` sections. When `development_team` is set, Once also derives the `application-identifier` entitlement from the team and bundle identifiers so application-group containers work in the simulator. The bundle is then signed without those platform entitlements, matching Swift Build and the Apple Bazel rules. Device applications pass the processed entitlement file to code signing instead.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `apple_linkable`, `apple_framework`, `apple_resource`, `apple_swift_plugin`, `native_linkable` | Libraries, frameworks, resources, native linkables, and Swift compiler plugins embedded in the app |

Transitive library resource bundles are embedded at the application root.
Interface files and managed object models are compiled before each resource
bundle and the outer application are signed.

## Providers

The target emits `apple_application` and `apple_bundle`. Hosted test bundles
also receive the application's transitive Swift module directories, header
search directories, generated compatibility headers, framework inputs, and
virtual file-system overlays. This keeps an application's bridging-header
environment intact when the test compiler imports its testable module.
The application provider also exposes its executable so a hosted test bundle
can use it as the linker's bundle loader.

## Capabilities

| Capability | Output groups | Requires |
| --- | --- | --- |
| `build` | `default`, `bundle`, `dsyms` |  |
| `run` | `default` | `bundle` |

## Outputs

| Output | Location |
| --- | --- |
| Application bundle | `.once/out/<target>/<product_name>.app` |
| Executable | `.once/out/<target>/<product_name>.app/<product_name>` |
| Swift compatibility header | `.once/out/<target>/<module_name>-Swift.h` for mixed Swift and Objective-C targets |
| Property list | `.once/out/<target>/<product_name>.app/Info.plist` |
| Embedded frameworks | `.once/out/<target>/<product_name>.app/Frameworks` when the target depends on frameworks |
| Embedded resource bundles | `.once/out/<target>/<product_name>.app/*.bundle` when dependencies propagate resources |
| Code signature | `.once/out/<target>/<product_name>.app/_CodeSignature/CodeResources` |
| Run record | `.once/out/<target>/run/run.json` after `once run` |
| Run log | `.once/out/<target>/run/run.log` after `once run` |

## Running

`once run` launches macOS apps with the host app launcher and iOS
simulator apps with `simctl` boot, install, and launch. Pass
`once run --visible` to also open Simulator for the selected simulator
before installing and launching the app.

Each run writes a run record and log under the target's output directory.
Repeated runs launch the application again rather than replaying an
action-cache hit. Physical-device launch is unsupported.

## Limitations

Provisioning profiles, signing identities, and signing modes other than
`ad_hoc` are unsupported. The target rejects unsupported values instead of
ignoring them.

## Example

```toml
[[target]]
name = "Hello"
kind = "apple_application"
srcs = ["Sources/**/*.swift"]

[target.attrs]
platform = "ios"
bundle_id = "dev.once.Hello"
minimum_os = "17.0"
families = ["iphone"]
```
