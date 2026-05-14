# Apple and Swift

Fabrik supports granular Swift/macOS targets and a first-pass
`apple.simulator_app` target for Swift simulator apps.

## Swift Libraries and macOS Tools

`apple.swift_library` expands into two cacheable internal actions: one
Swift compile action that emits the module and object files, and one
archive action that turns those object files into a static archive.
`apple.macos_command_line_application` links Swift sources and Swift
library deps into a macOS executable.

```toml
[[apple.swift_library]]
name = "Greeter"
srcs = ["Sources/Greeter.swift"]
module_name = "Greeter"
minimum_os = "15.0"

[[apple.macos_command_line_application]]
name = "hello"
srcs = ["Sources/main.swift"]
deps = ["Greeter"]
module_name = "Hello"
minimum_os = "15.0"
```

Build the executable:

```sh
fabrik build examples/apple/macos/cli/hello
```

The compile, archive, and executable steps are separately cacheable. A
source change in the library invalidates its compile action, archive
action, and reverse deps; a source change in the executable leaves the
library cache entries reusable.

Swift actions use a narrow, deterministic environment and pass path
remapping flags so cached `.swiftmodule` outputs do not embed the local
project path. Debug option serialization is disabled for cached Swift
modules, matching the shape needed for remote cache reuse.

Fabrik also has `apple.static_framework` and `apple.dynamic_framework`
TOML targets for Swift framework bundles:

```toml
[[apple.static_framework]]
name = "Greeter"
srcs = ["Sources/Greeter.swift"]
module_name = "Greeter"
bundle_id = "dev.fabrik.Greeter"
```

## Simulator Apps

`apple.simulator_app` builds a simulator `.app` bundle. The only
supported platform today is `ios`; the target name leaves room for
watchOS and tvOS simulator apps without introducing a second concept.

```toml
[[apple.simulator_app]]
name = "Demo"
platform = "ios"
bundle_id = "dev.fabrik.ios-demo"
srcs = ["Sources/App.swift"]
minimum_os = "17.0"
```

Build the app bundle:

```sh
fabrik build examples/apple/ios/simulator-app/Demo
```

Launch it in a simulator:

```sh
fabrik run examples/apple/ios/simulator-app/Demo
```

Set `FABRIK_IOS_SIMULATOR` to a simulator UDID when you do not want to use the booted device:

```sh
FABRIK_IOS_SIMULATOR=<udid> fabrik run examples/apple/ios/simulator-app/Demo
```

## Cache Behavior

- Swift libraries, Swift frameworks, macOS command-line applications,
  and `apple.simulator_app` builds are cacheable.
- `fabrik run` first reuses the cacheable app build, then runs an uncached install and launch action.
- Simulator boot, install, and launch are runtime side effects and are intentionally not cached.

## Dependency Sync

Declare Swift dependencies in the root `fabrik.toml` and run
`fabrik deps sync` to refresh the generated Swift dependency graph.

```toml
[[deps]]
name = "swiftpm"
ecosystem = "swift"
manifest = "Package.swift"
lockfile = "Package.resolved"
output = "vendor/fabrik.swift.lock.json"

[[apple.swift_library]]
name = "CLI"
srcs = ["Sources/CLI.swift"]
deps = [
  { swiftpm = { product = "ArgumentParser", package = "swift-argument-parser" } },
]
```

Run it:

```sh
fabrik deps sync swiftpm
```

The Swift sync step reads the declared `Package.resolved` file and emits
a lock graph JSON file. It records package identity, version, revision,
checksum, and git or registry source data where SwiftPM includes it.
The inline table entries in `deps` are external dependency edges: the
key points to the named `[[deps]]` graph, and the value is interpreted
by the SwiftPM adapter. Swift does not yet generate granular Swift
targets from SwiftPM packages.

## Current Limits

- Swift support currently targets host-architecture macOS builds.
- SwiftPM package graph sync records resolved dependencies, but it
  does not yet lower packages into buildable Fabrik targets.
- Swift target declarations preserve `{ swiftpm = ... }` external
  dependency edges, but build actions do not yet compile SwiftPM
  products into module or link inputs.
- Simulator app dependencies are not wired yet.
- Apple resource processing, asset catalogs, entitlements, and signing
  beyond simulator ad hoc signing are still future work.
