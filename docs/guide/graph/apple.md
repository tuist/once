# Apple Graph

Build Swift, Objective-C, C, and C++ libraries, frameworks, applications,
and test bundles from declarative `once.toml` targets. `apple_library`
drives `xcrun`-backed `swiftc` and `clang` on macOS to produce real static
archives, Swift modules, ObjC interop headers, and (when requested) clang
modulemaps. `apple_framework`, `apple_application`, and `apple_test_bundle`
declare the bundle, app, and test-host kinds and run through the same
action cache.

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

- `apple_library`: Swift, Objective-C, C, and C++ static library with
  Swift module emission, bridging headers, modulemap generation,
  `exported_deps`, transitive provider propagation, and configurable
  Xcode and SDK selection.
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
`-fmodule-map-file=<path>` automatically.

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
  ignores it).
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

## Not yet implemented

- **Multi-arch and `lipo`**: targets compile against the host arch
  only; universal archives and Mac Catalyst wait on a per-arch fan-out.
- **`select()` / configurable attributes / platform transitions**:
  attrs are marked `configurable = True` in the prelude, but there is
  no `select()` runtime yet, so the flag is aspirational. This is the
  platform model epic.
- **Swift macros (`plugins`)**: blocked on a `swift_macro` rule that
  produces the loadable plugin binaries.
- **Header maps (`.hmap`)**: pure `module.modulemap` covers the
  common path; the binary `.hmap` writer is future work.

## Agent workflows (planned)

The action cache and content-addressed storage already give every
action a stable digest and addressable outputs. A planned MCP
interface will expose this surface so coding agents can call a graph
action with a returned id and later query the cached outputs, logs,
and provider records by that id without re-running anything. Schema
introspection, structured diagnostics, and the
[`once query`](/reference/cli/query) commands above are designed to
feed that surface.

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
