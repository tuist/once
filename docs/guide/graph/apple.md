# Apple

Once builds Swift, Objective-C, C, and C++ targets for Apple
platforms from declarative `once.toml` manifests.
[`apple_library`](/reference/prelude/apple_library) drives
`xcrun`-backed `swiftc` and `clang` on macOS to produce real static
archives, Swift modules, ObjC interop headers, modulemaps, and
header maps. [`swift_macro`](/reference/prelude/swift_macro) compiles
Swift compiler-plugin dylibs that `apple_library` deps pick up
automatically. The bundle, app, and test-host kinds
([`apple_framework`](/reference/prelude/apple_framework),
[`apple_application`](/reference/prelude/apple_application),
[`apple_test_bundle`](/reference/prelude/apple_test_bundle)) are
implemented as Starlark graph target kinds that declare cacheable build
actions. Runtime effects, such as launching an app, are also declared
by target kinds and can opt out of action-cache replay.

For the per-target-kind attribute, dep, provider, and capability tables see
the [target kind reference](/reference/prelude/).

## Targets

Package manifests declare targets with one canonical shape:

```toml
[[target]]
name = "AppCore"
kind = "apple_library"
srcs = ["Sources/**/*.swift"]

[target.attrs]
platform = "ios"
minimum_os = "17.0"
sdk_frameworks = ["UIKit"]
```

Dependency references are root-relative by default. `./` and `../`
references resolve from the package that owns the manifest.

Apple targets can also depend on native providers from other ecosystems.
[`kotlin_apple_framework`](/reference/prelude/kotlin_apple_framework)
emits an Apple framework provider that application and test targets can
link and embed. [`rust_mobile_library`](/reference/prelude/rust_mobile_library)
emits archive fields that Apple link targets consume through the same
`deps` edge.

## Commands

Inspect the graph with [`once query`](/reference/cli/query):

```sh
once query targets
once query capabilities apps/ios/AppCore
once query schema apple_library
```

`once query schema <kind>` returns the same typed contract the
[target kind reference](/reference/prelude/) documents: which attributes
a target of that kind accepts, which providers each dep edge expects,
which providers the target kind emits, and which capabilities it exposes.

Materialize a target with the [`once build`](/reference/cli/build)
capability:

```sh
once build apps/ios/AppCore
```

Outputs land under `.once/out/<target>/`. The per-target-kind reference
pages list the exact paths each target kind emits.

## Running Apps

`apple_application` produces an `.app` bundle and exposes `run`.
`once run` first materializes the required bundle output, then executes
the launch action declared by the Apple target kind. That launch action is not
cacheable, so each `once run` attempts a fresh launch instead of
replaying an action-cache hit.

The Apple target kind owns the platform behavior. macOS apps are launched with
the host app launcher. iOS simulator apps use `simctl` to pick or boot a
simulator, install the bundle, and launch the bundle identifier. Device
launch support is not implemented yet.

Pass `--visible` to open Simulator for the selected simulator before the
install and launch steps:

```sh
once build apps/ios/App
once run --visible apps/ios/App
```

## Configurable attributes

Some attribute values depend on what you're building for. A library
might link `UIKit` on iOS and `AppKit` on macOS, or pick different
linker flags per architecture. Instead of duplicating the target,
write the per-configuration value inline with `select`:

```toml
[target.attrs]
sdk_frameworks = { select = { ios = ["UIKit"], macos = ["AppKit"] } }
```

When the build resolves the target, it picks the branch whose key
matches the active configuration. A branch key can be:

- a platform: `ios`, `macos`, `tvos`, `watchos`, `visionos`
- an architecture from `archs`: `arm64`, `x86_64`, `arm64e`, ...
- an SDK variant: `simulator` or `device`
- the literal token `mac_catalyst` when `mac_catalyst = true`
- a combination joined with `:`, such as `ios:simulator`
- `default`, used when no other branch matches

When more than one branch matches (e.g. both `ios` and `ios:simulator`
on an iOS simulator build) the most specific one wins.

Two kinds of attributes cannot use `select`:

- The attributes that decide which branch is picked: `platform`,
  `sdk_variant`, `archs`, and `mac_catalyst`. They have to be literal
  values.
- Attributes the target kind marks as non-configurable. Trying to use
  `select` on those surfaces a graph loading error.

The surface is intentionally small for now. Richer configuration
models (constraint settings, exec/target splits, platform
transitions) can layer on later without changing the TOML shape.

## Prior art

The Apple target kind set adapts ideas from established Apple build tooling
rather than copying its surface:

- Bazel's Apple build support, where application target kinds handle
  linking and bundling while Swift and Objective-C compilation live in
  dedicated language target kinds.
- [Bazel apple_binary](https://docs.bazel.build/versions/3.0.0/be/objective-c.html#apple_binary),
  which exposes Apple binary concepts such as platform type, minimum
  OS version, SDK frameworks, SDK dylibs, weak SDK frameworks, link
  options, and multi-architecture outputs.
- Buck2's Apple build primitives, which separate compile inputs, Apple
  toolchain selection, bundle assembly, resources, asset catalogs,
  Info.plist values, entitlements, provisioning profiles, and tests.

Once is not Buck-compatible, Bazel-compatible, or a drop-in
replacement for either tool; users and agents declare targets in
`once.toml`, built-in target kind metadata lives in the Starlark prelude,
and the graph is intentionally inspectable first so agents and CLI
users can ask what a target can do before broad execution exists.
