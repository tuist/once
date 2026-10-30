# `swift_package_build_tools`

Swift package build-tool plugin execution and generated files.

## Description

Native Swift package and Xcode workspace resolvers create this dependency for
targets that declare build-tool plugins. Swift Package Manager builds the host
tools, evaluates plugins, and executes prebuild commands. Once schedules regular
build commands in dependency order, then compiles generated sources and packages
generated resources with the consuming Apple target. Declared generated headers
are available on that target's private header search paths.

Planning does not compile ordinary application products. Commands without
outputs run when the consuming package uses Swift tools version 6 or newer.
Command failures fail the build. Plugin planning and commands inherit the host
environment, execute locally without sandboxing, and bypass caching. Normal
compilation retains its content-addressed cache.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `package_path` | string | yes | | Absolute path of the consuming package |
| `target_name` | string | yes | | Package target that declares the plugin |
| `dependencies` | map&lt;string,string&gt; | no | `{}` | Resolver-owned JSON records keyed by package identity, each containing its checkout `path`, authoritative `pin`, and manifest `locations` |
| `per_language_clang_flags` | map&lt;string,string&gt; | no | `{}` | Resolver-owned JSON flag arrays keyed by `c` or `cxx`, applied to generated native sources |
| `traits` | list&lt;string&gt; | no | `[]` | Enabled package traits |
| `declares_traits` | bool | no | `false` | Whether the package declares traits, including an explicitly empty resolved selection |
| `build_role` | string | no | `destination` | `tools` for host consumers or `destination` for application consumers |
| `platform` | string | no | `macos` | Destination platform |
| `minimum_os` | string | no | `13.0` | Minimum destination operating system version |
| `sdk_variant` | string | no | `simulator` | Destination SDK variant |
| `configuration` | string | no | `Debug` | Names containing `debug` or `development`, case-insensitively, select debug; other names select release |
| `xcode_developer_dir` | string | no | | Xcode developer directory |

These attributes are not configurable. Prefer the native resolvers over
authoring this synthetic target directly.

## Providers and capabilities

The target exposes `build` and provides
[`apple_generated_files`](/reference/modules#apple-generated-files).
Consumers include declared outputs and discovered prebuild files, rather than
arbitrary files in the plugin's working directory.

## Compatibility

The integration is tested against Apple Swift 6.3.3. It uses SwiftPM's unsupported
multi-root and native planning interfaces, whose formats can change between
toolchains. Only the consuming package becomes a planning root. Source-control
dependencies use local mirrors and their authoritative locked versions and
revisions, while dependency traits remain package-scoped. A trait-enabled package
does not impose its traits on unrelated dependencies, and empty selections keep
default traits disabled. Host and destination consumers receive separate outputs.
Planning can evaluate other targets in the consuming package, including tests.
If those targets require dependencies absent from the authoritative lockfile,
planning fails instead of selecting new versions. Prebuild commands follow SwiftPM's restrictions, including
its requirement to use existing or binary-vended tools instead of source-built
executables. Regular build commands can use source-built tools.

## Sources

- [Swift package plugins](https://github.com/swiftlang/swift-package-manager/blob/main/Documentation/Plugins.md)
- [swift-build custom tasks](https://github.com/swiftlang/swift-build/blob/main/Sources/SWBTaskConstruction/TaskProducers/OtherTaskProducers/CustomTaskProducer.swift)
