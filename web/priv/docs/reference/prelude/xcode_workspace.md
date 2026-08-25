# `xcode_workspace`

Xcode project seed.

## Description

Reads an existing Xcode project or workspace and lowers every native target it
finds into the Apple target kinds, so Once compiles and tests the project
directly. The `.xcodeproj` remains the source of truth: targets, build
settings, and file membership are read from it rather than restated in
`once.toml`.

The resolver converts `project.pbxproj` with `plutil`, flattens layered build
settings from the project, the target, and any `.xcconfig` includes, resolves
file references from both classic build phases and Xcode 16 file-system
synchronized groups, reads schemes to identify testable targets, replays shell
script phases as prebuild actions, and lowers Swift package dependencies into
Apple libraries.

Pointing `project` at an `.xcworkspace` resolves every `.xcodeproj` the
workspace references and merges their targets into one graph, so dependencies
that cross a project boundary are wired. A referenced project that is not on
disk is skipped instead of failing the graph.

Xcode projects, including projects in nested packages, are recognized as
native projects named `xcode`, so their seeds are supplied without any
`once.toml`. Use `once native init xcode` to persist a selected seed.

When an Xcode project uses a workspace-level `Package.resolved`, Once uses its
matching pinned revisions while lowering remote Swift packages. Checksum-pinned
binary package archives download as normal cacheable dependencies instead of
being fetched while the graph loads.

See [Xcode Projects](/guide/graph/apple/xcode) for a walkthrough.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `project` | string | no | single `*.xcodeproj` in the package | Package-relative path to the `.xcodeproj` or `.xcworkspace` |
| `configuration` | string | no | `Debug` | Xcode build configuration whose settings drive target lowering |
| `sdk_variant` | string | no | `simulator` | `simulator` or `device` selection applied to lowered targets on non-macOS platforms |
| `xcode_developer_dir` | string | no | active Xcode | `DEVELOPER_DIR` override folded into lowered targets' cache keys |
| `binary_artifact_authorization_env` | string | no |  | Environment-variable name that supplies a web Authorization header while downloading private binary package archives. Its value is not recorded. |
| `resolver_inputs` | list&lt;string&gt; | no | `[]` | Package-relative text globs supplied to resolution. Defaults to `srcs` when empty |

None of these attributes are configurable by platform select.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `apple_linkable`, `apple_application`, `apple_test_bundle`, `native_linkable` | Native Xcode targets lowered into Apple application, library, framework, and test targets |

## Lowering

| Xcode product type | Emitted target kind |
| --- | --- |
| Application, iMessage app, App Clip | [`apple_application`](/reference/prelude/apple_application) |
| App extension, embedded watch app | [`apple_application`](/reference/prelude/apple_application) with `application_extension` |
| Framework | [`apple_framework`](/reference/prelude/apple_framework) |
| Static framework, static or dynamic library | [`apple_library`](/reference/prelude/apple_library) |
| Unit test and interface test bundle | [`apple_test_bundle`](/reference/prelude/apple_test_bundle) |
| Bundle | [`apple_resource_bundle`](/reference/prelude/apple_resource_bundle) |
| Referenced `.xcframework` | [`apple_xcframework_import`](/reference/prelude/apple_xcframework_import) |
| Remote binary Swift package target | [`archive_download`](/reference/prelude/archive_download), then [`apple_xcframework_import`](/reference/prelude/apple_xcframework_import) |

## Providers

The target emits `xcode_workspace`. The lowered targets emit the providers of
their own kinds, so downstream targets depend on them normally.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | none |

Building the seed builds every lowered target. Lowered targets can also be
built, run, and tested individually by name.

## Tools

| Tool | Executables |
| --- | --- |
| `xcode` | `plutil`, `xcrun` |

## Limitations

Resolver inputs are read as text, so `srcs` must not glob binary files.

Once compiles the project rather than delegating to `xcodebuild`, so anything
supplied by an external dependency manager or bootstrap step, including
CocoaPods build integration and vendored binaries a repository fetches, has to
be in place before the build.

App extensions and embedded watch apps compile as application bundles. The
`.appex` wrapper and its extension-point metadata are not modeled yet.

Targets without compilable sources are skipped, and dependency edges to targets
that were not emitted are dropped so the remaining graph stays loadable.

## Example

```toml
[[target]]
name = "App"
kind = "xcode_workspace"
srcs = ["App.xcodeproj/project.pbxproj"]

[target.attrs]
project = "App.xcodeproj"
```
