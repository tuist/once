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

The `artifact` capability packages the completed application as a ZIP. The
provider adds `once_artifact_info` with one `product` member and a `bundle`
payload. `dev.once.apple-install.v1` describes the platform, SDK variant,
architecture, bundle identifier and minimum operating system. Packaging records
declared executable permissions and preserves symlinks.

On macOS, `dev.once.apple-install.v2` also points to `bundle` and names a
`metadata` payload through `spec.metadata_payload`. Its
`once.apple_application.v1` document records `display_name`, `bundle_identifier`,
`version`, `build_version`, `mach_o_uuids`, `platform`, `sdk_variant`, `signing`
and `signature_verified`. The producer verifies the completed bundle with
`codesign --verify --deep --strict` and collects Mach-O UUIDs with `dwarfdump`.

The v2 producer declaration approves automatic publication of the full closure
only for ad-hoc signed macOS and simulator applications exported on macOS.
Physical-device applications and Linux exports do not approve it. Builds do
not upload without separate workspace policy and local destination
authorization. See [Artifacts](/guide/infrastructure/artifacts#publish-after-successful-builds).
Device installation still requires provisioning and re-signing outside this flow.

macOS applications use the native `Contents` layout: executables in
`Contents/MacOS`, resources and dependency resource bundles in
`Contents/Resources`, and frameworks in `Contents/Frameworks`. The linker
includes `@executable_path/../Frameworks` automatically. Generated macOS
property lists use `LSMinimumSystemVersion` and omit iOS device-family keys.
The `macosx` platform alias uses the same layout. Other Apple platforms retain
their flat bundle layout.

Linux builds use Swift, xtool's Darwin SDK, and `rcodesign`. iOS device builds
use ARM64 even on x86_64 Linux hosts. The output remains ad-hoc signed and must
be provisioned and re-signed before installation on a physical device. The
`run` capability requires macOS. See [Build iOS Applications on
Linux](/guide/graph/apple/linux) for setup and a runnable starter.

## Attributes

`explicit_modules` is a boolean, defaulting to `false`, that enables
compiler-scanned, cacheable Swift and Clang module actions.
`dependency_check` accepts `"off"` (the default) or `"error"`; checking
requires explicit modules. See [explicit modules](/guide/graph/apple#explicit-modules-and-dependency-checks)
for native propagation, dependency errors, and current limitations.
The resolver-owned `_declared_deps` metadata preserves declarations before
import inference and should not be authored manually.

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `platform` | string | yes |  | Apple platform for the application |
| `bundle_id` | string | yes |  | Application bundle identifier |
| `minimum_os` | string | no | `13.0` | Minimum supported operating system version |
| `target_sdk_version` | string | no | `minimum_os` | Software development kit version used in the target triple |
| `sdk_variant` | string | no | `simulator` | `simulator` or `device`; ignored on macOS (not configurable) |
| `xcode_developer_dir` | string | no | active Xcode on macOS; registered `darwin` SDK on Linux | Xcode Developer directory on macOS or xtool Darwin SDK Developer directory on Linux |
| `families` | list&lt;string&gt; | no | `[]` | Supported device families (`iphone`, `ipad`); an empty list uses `iphone` |
| `product_name` | string | no | target name | Application product name (not configurable) |
| `module_name` | string | no | product name | Swift module name (not configurable) |
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
| `private_headers` | list&lt;string&gt; | no | `[]` | Private header files required while compiling the application |
| `bridging_header` | string | no |  | Objective-C bridging header imported into Swift sources |
| `prefix_header` | string | no |  | Prefix header included before every C-family source |
| `prebuild_actions` | list&lt;string&gt; | no | `[]` | Adapter-owned serialized build preparation actions that run before compilation. Records may opt into caching when they declare complete inputs and outputs; always-run records remain uncached |
| `application_extension` | bool | no | `false` | Build as an app extension: entered through `NSExtensionMain` and compiled against the app-extension-safe interface |
| `app_icon` | string | no |  | Asset catalog app-icon set name compiled into the application icon |
| `asset_symbols` | bool | no | `true` | Generate Swift asset symbols for the application's asset catalogs |
| `enable_testing` | bool | no | `false` | Compile Swift with testability enabled so hosted test bundles can use `@testable import` |

For simulator applications, processed platform entitlements are embedded in the executable's `__TEXT,__entitlements` and `__TEXT,__ents_der` sections. When `development_team` is set, Once also derives the `application-identifier` entitlement from the team and bundle identifiers so application-group containers work in the simulator. The bundle is then signed without those platform entitlements, matching Swift Build and the Apple Bazel rules. Device applications pass the processed entitlement file to code signing instead.

The `prepackage_actions` attribute accepts an ordered `list<string>` of
serialized script records, defaulting to `[]`. These run after linking and
before resource processing, so generated resources feed packaging.

The `postbuild_actions` attribute accepts an ordered `list<string>` of
serialized script records, defaulting to `[]`. These actions run after
product assembly and before final signing. Complete declarations may be
cached; untracked scripts rerun and publish changes to known product files.
See [native script phases](/docs/guide/graph/apple/xcode#script-build-phases)
for mapped build settings and file-list variables.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `apple_linkable`, `apple_framework`, `apple_resource`, `apple_swift_plugin`, `native_linkable` | Libraries, frameworks, resources, native linkables, and Swift compiler plugins embedded in the app |

Transitive library resource bundles are embedded at the application root on
iOS and in `Contents/Resources` on macOS.
Interface files and managed object models are compiled before each resource
bundle and the outer application are signed.

## Providers

The target emits `apple_application` and `apple_bundle`. Hosted test bundles
also receive the application's transitive Swift module directories and exact
module artifacts, header search directories, generated compatibility headers,
framework inputs, and virtual file-system overlays. This keeps an application's
bridging-header environment intact when the test compiler imports its testable
module.
The application provider also exposes its executable so a hosted test bundle
can use it as the linker's bundle loader.

## Capabilities

| Capability | Output groups | Requires |
| --- | --- | --- |
| `build` | `default`, `bundle`, `dsyms` |  |
| `run` | `default` | `bundle` |
| `artifact` | `default`, `artifact` |  |

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

The executable, property list, framework, resource bundle, and signature
locations above describe flat bundles. On macOS their respective paths are
`<product_name>.app/Contents/MacOS/<product_name>`,
`<product_name>.app/Contents/Info.plist`,
`<product_name>.app/Contents/Frameworks`,
`<product_name>.app/Contents/Resources/*.bundle`, and
`<product_name>.app/Contents/_CodeSignature/CodeResources`. Compiled asset
catalogs also live in `Contents/Resources`.

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

For Developer ID distribution, sign and notarize a separate copy, outside
Once's cached actions. See [macOS applications and distribution](/guide/graph/apple#macos-applications-and-distribution)
for a runnable starter and a credential-isolated release workflow. Never sign
or staple the cached application in place.

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
