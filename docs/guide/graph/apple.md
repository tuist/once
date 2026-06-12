# Apple Graph

Build Swift, Objective-C, C, and C++ static libraries from declarative
`once.toml` targets. `apple_library` drives `xcrun`-backed `swiftc` and
`clang` on macOS to produce real static archives, Swift modules, ObjC
interop headers, and (when requested) clang modulemaps. The bundle, app,
and test-host kinds (`apple_framework`, `apple_application`,
`apple_test_bundle`) are declared as inspectable schemas; their build
implementations are not yet wired up.

## Targets

Package manifests declare targets with one canonical shape:

```toml
[[target]]
name = "App"
kind = "apple_application"
deps = ["./AppCore"]

[target.attrs]
platform = "ios"
bundle_id = "dev.once.App"
minimum_os = "17.0"
families = ["iphone", "ipad"]
resources = ["Resources/**"]
asset_catalogs = ["Assets.xcassets"]
info_plist = "Info.plist"
entitlements = "App.entitlements"
provisioning_profile = "profiles/App.mobileprovision"
sdk_frameworks = ["UIKit"]
```

Dependency references are root-relative by default. `./` and `../`
references resolve from the package that owns the manifest.

## Rules

Implemented:

- `apple_library`: Swift, Objective-C, C, and C++ static library with
  Swift module emission, bridging headers, modulemap and header-map
  generation, `exported_deps`, transitive provider propagation, and
  configurable Xcode and SDK selection. Multi-arch builds fan out
  per-arch compiles and combine them with `lipo`; Mac Catalyst is
  available through `mac_catalyst = true`.
- `swift_macro`: Swift compiler-plugin dylib built for the host. Any
  `apple_library` dep edge that points at a `swift_macro` target picks
  up `-load-plugin-library <dylib>` automatically. The macro itself
  links against a swift-syntax checkout the user provides via `deps`.

Declared, awaiting implementation. The schema is inspectable via
`once query schema <kind>` and rejects malformed manifests, but no
build actions run yet:

- `apple_framework`: framework bundle metadata, resources, asset
  catalogs, privacy manifests, headers, and debug symbol outputs.
- `apple_application`: application bundle metadata, device families,
  resources, asset catalogs, Info.plist substitutions, entitlements,
  provisioning profile, signing policy, and SDK frameworks.
- `apple_test_bundle`: XCTest bundle metadata, optional test host,
  resources, asset catalogs, Info.plist, entitlements, destination,
  test plan, and test environment.

## Commands

Inspect the graph with [`once query`](/reference/cli/query):

```sh
once query targets
once query capabilities apps/ios/App
once query schema apple_application
```

`once query schema <kind>` returns the rule's typed contract: which
attributes a target of that kind accepts, which providers each dep edge
expects, which providers the rule emits, and which capabilities it
exposes.

Materialize a target with the build, run, and test
[capabilities](/reference/cli/build):

```sh
once build apps/ios/App
once run  apps/ios/App
once test apps/ios/AppTests
```

Outputs land under `.once/out/<target>/`. `apple_library` writes the
static archive, the Swift module triple (`.swiftmodule`, `.swiftdoc`,
generated `-Swift.h`), and any clang object files. `apple_application`
writes the `.app` bundle and dSYMs; `apple_test_bundle` writes test
results and coverage records.

## `apple_library` compile

`apple_library` routes every source file through the driver that
matches its extension:

- **Swift sources** go through `xcrun --sdk <sdk> swiftc
  -emit-library -static -emit-module`. A `bridging_header` plumbs in
  via `-import-objc-header` so Swift can see ObjC symbols.
- **ObjC, C, and C++ sources** each become an independent
  `xcrun --sdk <sdk> clang -c` action that writes one `.o` per
  source. The clang invocation pulls the SDK sysroot from
  `xcrun --show-sdk-path`, targets the active triple, and enables
  ARC for ObjC.
- **Mixed-language libraries** combine the Swift-only archive and
  the per-source clang objects with `xcrun libtool -static`.
  Swift-only or clang-only libraries skip the merge.

Dep `swiftmodule` directories are forwarded as `-I` search paths so
`import` statements resolve. With `enable_modules = true` the impl
writes a `module.modulemap` from `exported_headers` and propagates
it through the provider so consumers pick up
`-fmodule-map-file=<path>` automatically. Alongside the modulemap the
impl also writes a binary header map (`<module_name>.hmap`) that maps
each exported header's basename and `<module_name>/<basename>` form to
its workspace-relative path, and threads the hmap into both clang and
swiftc invocations through `-I`. This covers the `#include "Foo.h"`
and `#include <Module/Foo.h>` lookup styles a pure modulemap doesn't
help with.

The action cache key composes the resolved toolchain identity (each
of swiftc, clang, and libtool carries its own `xcrun`-resolved path,
version banner, and any `DEVELOPER_DIR` override), the source content
digests, and each dep's action digest. A swap of Xcode, a source
edit, or a transitive dep change invalidates exactly the affected
cache slots.

### `apple_library` attributes

- **Platform and triple**: `platform` (required), `minimum_os`
  (deployment target), `target_sdk_version` (build-time SDK; defaults
  to `minimum_os`), `sdk_variant` (`simulator` or `device`; macOS
  ignores it), `archs` (target architectures; empty defaults to the
  host arch, multi-arch fans out per-arch compiles and combines them
  with `lipo -create`), `mac_catalyst` (build the iOSMac variant;
  requires `platform = macos`).
- **Toolchain pin**: `xcode_developer_dir` overlays `DEVELOPER_DIR`
  on every `xcrun` call and folds into the cache identity.
- **Sources and headers**: `srcs` (globs), `headers`,
  `exported_headers`, `bridging_header`.
- **Compile flags**: `module_name`, `swift_flags`, `clang_flags`,
  `defines` (propagated transitively), `enable_testing`,
  `library_evolution`, `enable_modules`, `emit_dsym`.
- **Link inputs (propagated transitively)**: `sdk_frameworks`,
  `weak_sdk_frameworks`, `sdk_dylibs`, `linkopts`, `alwayslink`.
- **Dep edges**: `deps` (linked), `exported_deps` (linked and
  re-exposed to consumers at compile time).

### Provider record

`apple_library` returns a record consumers read through `ctx["deps"]`.
It carries:

- the direct outputs (`swiftmodule_dir`, `archive`, `objc_header`,
  `modulemap`);
- the headers this target re-exposes (`exported_headers`,
  `exported_header_dirs`);
- transitive lists for everything downstream rules will need to
  compose their own link line: archives, swiftmodule directories
  (gated by `exported_deps`), SDK frameworks, sdk dylibs, linkopts,
  defines, modulemaps, and the always-link archive subset.

The shape mirrors `SwiftInfo` and `CcInfo` from the Bazel rules so
existing build engineers have a familiar mental model.

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

Once is not Buck-compatible, Bazel-compatible, or a drop-in replacement
for either tool; users and agents declare targets in `once.toml`,
built-in rule metadata lives in the Starlark prelude, and the graph is
intentionally inspectable first so agents and CLI users can ask what a
target can do before broad execution exists.
