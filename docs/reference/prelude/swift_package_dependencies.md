# `swift_package_dependencies`

Locked Swift package graph imported as Once targets and exposed to Apple builds.

## Description

`swift_package_dependencies` treats [Swift Package Manager](https://www.swift.org/documentation/package-manager/) as the authority for package resolution. Once reads `Package.resolved`, imports each locked package as a `swift_package_pin`, preserves package-to-package edges, and attaches the direct pins to the dependency-set target.

The target accepts `Package.resolved` schemas 1, 2, and 3. A synthetic target name includes the canonical package identity and the first available immutable state in this order: registry checksum, source-control revision, semantic version, branch, or `local`. Changing a locked revision or checksum therefore changes the graph identity as well as the action key.

The build runs `swift build` with automatic resolution disabled. Selected static products become an `apple_linkable` provider, so `apple_library`, `apple_framework`, `apple_application`, and `apple_test_bundle` consume them through their ordinary `deps` edge.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `package_path` | string | no | `.` | Directory containing `Package.swift` and `Package.resolved` |
| `resolved_file` | string | no | `Package.resolved` | Lock file relative to `package_path`, imported during graph loading from `resolver_inputs`, or from `srcs` when resolver inputs are empty or omitted |
| `resolver_inputs` | list&lt;string&gt; | no | `srcs` | Package-relative text globs supplied to the resolver, excluding large vendored trees when set |
| `graph_file` | string | no |  | Checked-in [JavaScript Object Notation (JSON)](https://www.json.org/json-en.html) output relative to `package_path` from `swift package show-dependencies --format json`, with exact `once_manifest`, `once_resolved`, and `once_inputs` bindings |
| `vendor_path` | string | no |  | Swift Package Manager scratch tree containing checkouts, repositories, and workspace state |
| `allow_network` | bool | no | `false` | Allow live Swift package inspection when remote sources are not vendored |
| `products` | list&lt;string&gt; | yes for builds | `[]` | Root and transitive static library product names exposed to Apple linkers |
| `platform` | string | no | `macos` | Apple platform used for the package build |
| `minimum_os` | string | no | `13.0` | Minimum operating system version encoded in the target triple |
| `sdk_variant` | string | no | `simulator` | Simulator or device software development kit selection |
| `arch` | string | no | host architecture | Single target architecture |
| `configuration` | string | no | `release` | `debug` or `release` Swift package build configuration |
| `swift` | string | no | `swift` | Swift executable or workspace-relative executable path. The default selects the executable paired with the resolved Swift compiler |
| `xcode_developer_dir` | string | no |  | Specific Xcode developer directory used for Swift and the Apple software development kit |
| `build_flags` | list&lt;string&gt; | no | `[]` | Additional arguments appended to the locked package build |
| `alwayslink` | bool | no | `false` | Force-load every selected archive in downstream Apple links |
| `sdk_frameworks` | list&lt;string&gt; | no | `[]` | Apple software development kit frameworks propagated to consumers |
| `weak_sdk_frameworks` | list&lt;string&gt; | no | `[]` | Weakly linked Apple software development kit frameworks propagated to consumers |
| `sdk_dylibs` | list&lt;string&gt; | no | `[]` | Apple software development kit dynamic libraries propagated to consumers |
| `linkopts` | list&lt;string&gt; | no | `[]` | Additional linker flags propagated to consumers |
| `resolved_identities` | list&lt;string&gt; | resolver-owned | `[]` | Canonical package identities written by the resolver |
| `_remote_identities` | list&lt;string&gt; | resolver-owned | `[]` | Remote package identities used to enforce execution policy |
| `_locked_pins` | list&lt;string&gt; | resolver-owned | `[]` | Immutable package state included in native build action identities |

## Dependency edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `swift_package_pin` | Direct locked packages emitted by the resolver |

Manifest authors do not populate this edge. The resolver derives it from the root of the Swift package graph.

## Providers

The target emits `swift_package_dependencies`, `apple_linkable`, and `apple_module`.

The Apple provider exposes:

- one archive path for each name in `products`;
- the Swift module directory produced by the package build;
- configured frameworks, dynamic libraries, and linker options;
- the canonical locked identities for graph inspection.

Every root and transitive product needed by the consumer must appear in `products` and be declared as a static library product in `Package.swift`. Automatic library products do not guarantee a static archive path, so Once does not guess one. Packages that need resource-bundle embedding or build-tool plugin execution should remain behind a first-party static package facade until those artifacts have typed Apple provider fields.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `default`, `binary`, `swiftmodule` |

## Reproducible and offline operation

For deterministic graph loading, check in a graph snapshot and name it with
`graph_file`. Add `once_manifest` containing the exact `Package.swift` text and
`once_resolved` containing the exact `Package.resolved` text. Add `once_inputs`
as a map containing every exact resolver input except `graph_file` itself.
Include the manifest for every workspace-local package so local dependency
edge changes invalidate the snapshot.
Once then imports the snapshot without executing Swift Package Manager during
analysis. Both bindings must match, every locked pin must appear in the
snapshot, and every unlocked node must resolve to a workspace-local path.
Otherwise graph loading fails as stale. Each immutable pin record also
participates in the native build action identity.

For network-independent actions, use local path dependencies or provide a complete `vendor_path` containing the Swift Package Manager checkouts, repositories, and workspace state for the lock file. Once copies that tree into the target output directory before building, so the checked-in vendor state is never mutated. The action disables automatic resolution and the shared dependency cache, then uses a target-local manifest cache.

If the lock file contains a remote package and `vendor_path` is absent, graph loading requires a checked-in `graph_file` and building fails before Swift Package Manager starts. Set `allow_network = true` only when live inspection of an unvendored remote package is intentional. A vendored build copies the scratch tree before use, but Once does not independently prevent Swift Package Manager from reaching the network if that tree is incomplete. Automatic version selection remains disabled, and the immutable lock state participates in the Once graph and action key.

`vendor_path` requires `graph_file`. This keeps read-only graph loading from
writing workspace state or manifest caches into the checked-in vendor tree.

The default `swift` setting resolves the executable beside the selected Swift compiler. This prevents a package module from being compiled by a different Swift version than its first-party Apple consumer. Set `swift` explicitly only when intentionally supplying another compatible toolchain.

## Example

```toml
[[target]]
name = "Packages"
kind = "swift_package_dependencies"
srcs = [
  "Package.swift",
  "Package.resolved",
  "swiftpm-dependencies.json",
  "Sources/**/*.swift",
  "Vendor/SwiftPM/**",
]

[target.attrs]
resolver_inputs = [
  "Package.swift",
  "Package.resolved",
  "swiftpm-dependencies.json",
]
graph_file = "swiftpm-dependencies.json"
vendor_path = "Vendor/SwiftPM"
products = ["WorkspaceDependencies"]
platform = "ios"
minimum_os = "17.0"
sdk_variant = "simulator"

[[target]]
name = "Feature"
kind = "apple_library"
srcs = ["Feature/**/*.swift"]
deps = ["./Packages"]

[target.attrs]
platform = "ios"
minimum_os = "17.0"
```

The bundled starter uses a local package dependency, an empty version 3 lock file, and a checked-in dependency graph. It loads and builds without registry or source-control access.

Continue with the [Swift Packages guide](/guide/graph/swift-packages) for the
complete locked graph, local application, query, build, and update workflow.

## Upstream contracts

The importer follows Swift Package Manager's [`ResolvedPackagesStore`](https://github.com/swiftlang/swift-package-manager/blob/main/Sources/PackageGraph/ResolvedPackagesStore.swift) lock-file model and [`ShowDependencies`](https://github.com/swiftlang/swift-package-manager/blob/main/Sources/Commands/PackageCommands/ShowDependencies.swift) graph output.
