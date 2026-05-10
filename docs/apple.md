# Apple and Swift

Fabrik supports granular Swift/macOS targets and a first-pass
`apple.ios_app` target for Swift iOS simulator apps.

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
deps = ["//examples/macos-cli:Greeter"]
module_name = "Hello"
minimum_os = "15.0"
```

Build the executable:

```sh
fabrik build //examples/macos-cli:hello
```

The compile, archive, and executable steps are separately cacheable. A
source change in the library invalidates its compile action, archive
action, and reverse deps; a source change in the executable leaves the
library cache entries reusable.

Swift actions use a narrow, deterministic environment and pass path
remapping flags so cached `.swiftmodule` outputs do not embed the local
workspace path. Debug option serialization is disabled for cached Swift
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

## iOS Simulator Apps

`apple.ios_app` builds a simulator `.app` bundle.

```toml
[[apple.ios_app]]
name = "Demo"
bundle_id = "dev.fabrik.ios-demo"
srcs = ["Sources/App.swift"]
minimum_os = "17.0"
```

Build the app bundle:

```sh
fabrik build //examples/apple/ios/simulator-app:Demo
```

Launch it in a simulator:

```sh
fabrik run //examples/apple/ios/simulator-app:Demo
```

Set `FABRIK_IOS_SIMULATOR` to a simulator UDID when you do not want to use the booted device:

```sh
FABRIK_IOS_SIMULATOR=<udid> fabrik run //examples/apple/ios/simulator-app:Demo
```

## Cache Behavior

- Swift libraries, Swift frameworks, macOS command-line applications,
  and `apple.ios_app` builds are cacheable.
- `fabrik run` first reuses the cacheable app build, then runs an uncached install and launch action.
- Simulator boot, install, and launch are runtime side effects and are intentionally not cached.

## Current Limits

- Swift support currently targets host-architecture macOS builds.
- SwiftPM package resolution/import is not wired yet. The intended shape
  is to use `Package.swift` for external package resolution while keeping
  the local build graph declared with Fabrik's lower-level TOML targets.
- iOS app dependencies are not wired yet.
- Apple resource processing, asset catalogs, entitlements, and signing
  beyond simulator ad hoc signing are still future work.
