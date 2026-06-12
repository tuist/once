# Apple Graph

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
implemented as Starlark graph rules that declare cacheable build
actions.

For the per-rule attribute, dep, provider, and capability tables see
the [Prelude reference](/reference/prelude/).

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

## Commands

Inspect the graph with [`once query`](/reference/cli/query):

```sh
once query targets
once query capabilities apps/ios/AppCore
once query schema apple_library
```

`once query schema <kind>` returns the same typed contract the
[Prelude reference](/reference/prelude/) documents: which attributes
a target of that kind accepts, which providers each dep edge expects,
which providers the rule emits, and which capabilities it exposes.

Materialize a target with the [`once build`](/reference/cli/build)
capability:

```sh
once build apps/ios/AppCore
```

Outputs land under `.once/out/<target>/`. The per-rule reference
pages list the exact paths each rule emits.

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
- Attributes the rule marks as non-configurable. Trying to use
  `select` on those surfaces a graph loading error.

The surface is intentionally small for now. Richer configuration
models (constraint settings, exec/target splits, platform
transitions) can layer on later without changing the TOML shape.

## Prior art

The Apple rule set adapts ideas from established Apple build tooling
rather than copying its surface:

- [Bazel rules_apple](https://github.com/bazelbuild/rules_apple), where
  application rules handle linking and bundling while Swift and
  Objective-C compilation live in dedicated language rules.
- [Bazel apple_binary](https://docs.bazel.build/versions/3.0.0/be/objective-c.html#apple_binary),
  which exposes Apple binary concepts such as platform type, minimum
  OS version, SDK frameworks, SDK dylibs, weak SDK frameworks, link
  options, and multi-architecture outputs.
- [Buck2 apple_library](https://buck2.build/docs/prelude/rules/apple/apple_library/),
  [apple_binary](https://buck2.build/docs/prelude/rules/apple/apple_binary/),
  and [apple_bundle](https://buck2.build/docs/prelude/rules/apple/apple_bundle/),
  which separate compile inputs, Apple toolchain selection, bundle
  assembly, resources, asset catalogs, Info.plist values,
  entitlements, provisioning profiles, and tests.

Once is not Buck-compatible, Bazel-compatible, or a drop-in
replacement for either tool; users and agents declare targets in
`once.toml`, built-in rule metadata lives in the Starlark prelude,
and the graph is intentionally inspectable first so agents and CLI
users can ask what a target can do before broad execution exists.
