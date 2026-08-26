# `apple_test_bundle`

Apple test bundle.

## Description

Builds Apple test targets and runs XCTest or Swift Testing tests through the
generic Once test capability. Attributes whose names contain `sdk` configure
the
[Apple software development kit (SDK)](https://developer.apple.com/documentation/xcode)
used for the build.

Tests do not need to repeat framework dependencies that belong to a plugin or
library in their dependency graph. Once links only the direct framework
boundary and stages the full runtime framework closure inside the test bundle
before signing it.

Resources follow Xcode bundle placement. iOS test resources are placed at the
root of the test bundle, while macOS test resources are placed under
`Contents/Resources`. When `resource_bundle_name` is set, resources are
instead packaged into a named resource bundle inside the test product,
matching Swift Package Manager resource lookup. Structured resource roots preserve their own
directory name. Custom property-list templates can expand build-setting
placeholders, including the absolute source root supplied by an Xcode project
adapter.

The compiler receives both the XCTest framework search path and the platform
developer library search path. The latter contains XCTest's Swift module
overlay and its Swift support library, which are required for Swift assertion
helpers such as `XCTFail`.

When the dependency graph contains one non-extension application provider, the
test linker uses that application's executable as its bundle loader. This resolves code
under test from the host without copying the host's application objects into
the test bundle.

Interface tests use the platform test-runner application. Once packages and
signs the runner, nests the interface-test bundle under its `PlugIns`
directory, stages the required testing frameworks, and launches the declared
application under test. Compilation, linking, packaging, and signing remain
Once build actions.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `platform` | string | yes |  | Apple platform for the tests |
| `minimum_os` | string | no | `13.0` | Minimum supported operating system version |
| `target_sdk_version` | string | no | `minimum_os` | Software development kit version used in the target triple |
| `sdk_variant` | string | no | `simulator` | `simulator` or `device`; ignored on macOS (not configurable) |
| `xcode_developer_dir` | string | no | active Xcode | Xcode developer directory used to resolve build tools |
| `product_name` | string | no | target name | Test bundle product name (not configurable) |
| `module_name` | string | no | product name | Swift module name (not configurable) |
| `bundle_id` | string | no | `dev.once.tests.<product_name>` | Test bundle identifier (not configurable) |
| `test_host` | target | no |  | Application target hosting the test bundle |
| `resources` | list&lt;string&gt; | no | `[]` | Resource glob patterns bundled into the test bundle |
| `structured_resources` | list&lt;string&gt; | no | `[]` | Resource directory roots whose own basename is preserved inside the test bundle |
| `resource_bundle_name` | string | no |  | Optional resource bundle name; `.bundle` is added when omitted |
| `resource_bundle_id` | string | no |  | Bundle identifier written to generated resource bundle metadata |
| `asset_catalogs` | list&lt;string&gt; | no | `[]` | Asset catalog paths compiled into the test bundle |
| `info_plist` | string | no |  | Info.plist template path |
| `info_plist_substitutions` | map&lt;string,string&gt; | no | `{}` | Build-setting values substituted into property-list placeholders |
| `entitlements` | string | no |  | Entitlements plist path |
| `destination` | string | no |  | Simulator, device, or local destination selector |
| `test_plan` | string | no |  | XCTest plan path |
| `test_env` | map&lt;string,string&gt; | no | `{}` | Environment variables passed to the test runner |
| `test_arguments` | list&lt;string&gt; | no | `[]` | Arguments passed to the test process |
| `skipped_tests` | list&lt;string&gt; | no | `[]` | Suite or case identifiers excluded from the test run |
| `sdk_frameworks` | list&lt;string&gt; | no | `[]` | Apple software development kit frameworks linked by name |
| `weak_sdk_frameworks` | list&lt;string&gt; | no | `[]` | Apple software development kit frameworks linked weakly |
| `sdk_dylibs` | list&lt;string&gt; | no | `[]` | Apple software development kit dynamic libraries linked by name |
| `linkopts` | list&lt;string&gt; | no | `[]` | Extra linker flags |
| `swift_flags` | list&lt;string&gt; | no | `[]` | Extra Swift compiler flags |
| `clang_flags` | list&lt;string&gt; | no | `[]` | Extra Clang compiler flags for C, C++, Objective-C, and Objective-C++ test sources |
| `per_source_clang_flags` | map&lt;string,string&gt; | no | `{}` | JSON-encoded Clang compiler flag lists keyed by test source path |
| `defines` | list&lt;string&gt; | no | `[]` | Compatibility conditions passed to both Swift and Clang |
| `swift_defines` | list&lt;string&gt; | no | `[]` | Swift conditional compilation conditions |
| `clang_defines` | list&lt;string&gt; | no | `[]` | C-family preprocessor definitions |
| `exported_header_dirs` | list&lt;string&gt; | no | `[]` | Header search directories exported by the test target |
| `private_header_dirs` | list&lt;string&gt; | no | `[]` | Private header search directories used while compiling tests |
| `bridging_header` | string | no |  | Objective-C bridging header imported into Swift test sources |
| `prefix_header` | string | no |  | Prefix header included before every C-family test source |
| `prebuild_actions` | list&lt;string&gt; | no | `[]` | Adapter-owned serialized build preparation actions that run before compilation. Records may opt into caching when they declare complete inputs and outputs; always-run records remain uncached |
| `swift_testing` | bool | no | `false` | Run sources that use Swift Testing (`import Testing`) through the generic Once test capability |
| `ui_testing` | bool | no | `false` | Package the bundle inside the platform test runner and launch an application under test |
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
| C-family objects | `.once/out/<target>/Objects/*.o` for mixed-language test targets |
| macOS test binary | `.once/out/<target>/<product_name>.xctest/Contents/MacOS/<product_name>` |
| Other Apple platform test binary | `.once/out/<target>/<product_name>.xctest/<product_name>` |
| iOS interface-test runner | `.once/out/<target>/<product_name>-Runner.app` |
| iOS interface-test bundle | `.once/out/<target>/<product_name>-Runner.app/PlugIns/<product_name>.xctest` |
| macOS property list | `.once/out/<target>/<product_name>.xctest/Contents/Info.plist` |
| Other Apple platform property list | `.once/out/<target>/<product_name>.xctest/Info.plist` |
| macOS resources | `.once/out/<target>/<product_name>.xctest/Contents/Resources` |
| Other Apple platform resources | `.once/out/<target>/<product_name>.xctest` |
| Named macOS resource bundle | `.once/out/<target>/<product_name>.xctest/Contents/Resources/<resource_bundle_name>.bundle` |
| Named resource bundle on other Apple platforms | `.once/out/<target>/<product_name>.xctest/<resource_bundle_name>.bundle` |
| Compiled asset catalog | `Assets.car` in the platform-specific resource location |
| Runtime frameworks | The test bundle's `Frameworks` directory when dependencies require them |
| macOS code signature | `.once/out/<target>/<product_name>.xctest/Contents/_CodeSignature/CodeResources` |
| Other Apple platform code signature | `.once/out/<target>/<product_name>.xctest/_CodeSignature/CodeResources` |
| Test results | `.once/out/<target>/test/test_results.json` after `once test` |
| Test log | `.once/out/<target>/test/swift-testing.log` for Swift Testing or `xctest.log` for XCTest |
| Native runner output | `.once/out/<target>/test/native_results.txt` |

## Limitations

Direct `test_host` attributes, entitlements, destinations, and test plans are
unsupported. Application hosts discovered through dependency providers are
supported. Test execution is limited to macOS logic tests, iOS simulator unit
tests, and iOS simulator interface tests.

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
