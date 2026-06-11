# Apple Graph

Build Swift, Objective-C, C, and C++ libraries, frameworks, applications,
and test bundles from declarative `once.toml` targets. The Apple rules are
the first kinds Once's typed build graph ships; today `apple_library`
produces real static archives and Swift modules via `xcrun`-backed
`swiftc` on macOS, with `apple_framework`, `apple_application`, and
`apple_test_bundle` filling in as their toolchain plumbing lands.

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

Dependency references are root-relative by default. `./` and `../` references
resolve from the package that owns the manifest.

## Built-In Apple Rules

The initial built-in rules are defined in `crates/once-frontend/prelude/apple.star`:

- `apple_library`: Swift, Objective-C, C, and C++ sources, headers, exported
  headers, Swift and Clang flags, SDK frameworks, weak SDK frameworks, SDK
  dylibs, link options, testability, and library evolution.
- `apple_framework`: framework bundle metadata, resources, asset catalogs,
  privacy manifests, headers, exported headers, SDK frameworks, weak SDK
  frameworks, and debug symbol outputs.
- `apple_application`: application bundle metadata, device families, resources,
  asset catalogs, Info.plist substitutions, entitlements, provisioning profile,
  signing policy, SDK frameworks, weak SDK frameworks, and SDK dylibs.
- `apple_test_bundle`: XCTest bundle metadata, optional test host, resources,
  asset catalogs, Info.plist, entitlements, destination, test plan, and test
  environment.

## Commands

Use queries to inspect the graph:

```sh
once query targets
once query capabilities apps/ios/App
once query schema apple_application
```

Use capability commands to materialize the current graph contract:

```sh
once build apps/ios/App
once run apps/ios/App
once test apps/ios/AppTests
```

These commands run local cache-backed actions. `build` writes artifacts under
`.once/out/<target>/`, `run` consumes the built application bundle and writes a
run record, and `test` consumes the built test bundle and writes test results
and coverage records. For example, an `apple_application` build produces a
`.app` bundle and `dsyms`, while `run` depends on the `bundle` output. An
`apple_test_bundle` test produces `test_results` and `coverage`, and also
depends on the built bundle.

For `apple_library`, `build` produces a real static archive plus a Swift
module triple (`.swiftmodule`, `.swiftdoc`, `-Swift.h`). The rule supports
mixed-language sources:

- `.swift` sources go through `xcrun --sdk <sdk> swiftc -emit-library -static
  -emit-module`. If a `bridging_header` is set, swiftc gets
  `-import-objc-header` so Swift sources can see ObjC symbols.
- `.m`/`.mm`/`.c`/`.cc`/`.cpp`/`.cxx` sources each become a separate
  `xcrun --sdk <sdk> clang -c` (or `clang++`) action producing a per-source
  `.o`. The clang invocation uses the SDK sysroot from `xcrun --show-sdk-path`,
  the active target triple, and ARC (`-fobjc-arc`) for ObjC.
- If both Swift and clang sources are present, swiftc emits an intermediate
  `<Module>-swift.a` and `xcrun libtool -static` merges it with the clang
  objects into the final `<Module>.a`. Swift-only and clang-only libraries
  emit the final archive directly without the merge step.

The driver builds dep targets first and forwards each dep's swiftmodule
directory through `-I` (with `-Xcc -I` for the underlying Clang) so `import`
statements resolve. When `enable_modules = true`, the impl writes a
`module.modulemap` from `exported_headers` and propagates it through
`transitive_modulemaps`; consumers pick up `-fmodule-map-file=<path>`
automatically.

The action cache key composes the resolved toolchain identity (swiftc,
clang, or libtool; each carries its own `xcrun`-resolved path, version
banner, and any `DEVELOPER_DIR` override), the source content digests,
and the dep action digests. A swap of Xcode, a source edit, or a
transitive dep change each invalidates exactly the affected cache
slots.

### `apple_library` attribute reference

- **Platform + triple**: `platform` (required), `minimum_os` (deployment
  target), `target_sdk_version` (build-time SDK; defaults to `minimum_os`),
  `sdk_variant` (`simulator` or `device`; macOS ignores it).
- **Toolchain pin**: `xcode_developer_dir` overlays `DEVELOPER_DIR` on every
  `xcrun` call and is folded into the action cache identity.
- **Sources + headers**: `srcs` (globs), `headers`, `exported_headers`,
  `bridging_header`.
- **Module compile flags**: `module_name`, `swift_flags`, `clang_flags`,
  `defines` (propagated transitively), `enable_testing`, `library_evolution`,
  `enable_modules`, `emit_dsym`.
- **Link inputs (propagated transitively)**: `sdk_frameworks`,
  `weak_sdk_frameworks`, `sdk_dylibs`, `linkopts`, `alwayslink`.
- **Dep edges**: top-level `deps` (linked), `exported_deps` (linked AND
  re-exposed to consumers' compile path; matches Buck2's privacy boundary).

### Provider record returned by `apple_library`

`SwiftInfo` / `CcInfo`-shaped record consumed by downstream rules:

| field | what it carries |
| --- | --- |
| `label_id`, `swiftmodule_dir`, `archive`, `objc_header`, `alwayslink`, `modulemap` | Direct outputs for the immediate consumer |
| `exported_headers`, `exported_header_dirs` | Header surface this target re-exposes |
| `transitive_swiftmodule_dirs` | Only via `exported_deps` (Buck2 privacy) |
| `transitive_archives`, `transitive_alwayslink_archives` | Full link inputs |
| `transitive_sdk_frameworks`, `transitive_weak_sdk_frameworks`, `transitive_sdk_dylibs` | Frameworks / dylibs the linker reads |
| `transitive_linkopts`, `transitive_defines` | Flags + preprocessor macros that flow |
| `transitive_exported_headers`, `transitive_exported_header_dirs`, `transitive_modulemaps` | Compile-time header & module-map plumbing |

### Not yet implemented (tracking)

- **Multi-arch + `lipo`**: targets compile against the host arch only.
  Universal archives, Mac Catalyst, and pre-built SDK distribution are
  blocked on a per-arch fan-out.
- **`select()` / configurable attributes / platform transitions**: the
  schema marks attrs `configurable = True` but there is no `select()`
  runtime yet, so the flag is aspirational. This is the platform model
  epic and is bigger than every per-rule item combined.
- **Swift macros (`plugins`)**: needs its own `swift_macro` rule to produce
  the plugin `.dylib`s before they can be loaded via
  `-load-plugin-executable`.
- **Header maps (`.hmap`)**: the binary `.hmap` format needs a dedicated
  writer; pure `module.modulemap` covers the common path for now.

## Rule Implementations

Rule logic lives in the starlark prelude alongside the schema. Each rule
optionally declares an `impl = <callable>` that the analysis pass invokes with
a `ctx` describing the target. `ctx["attr"]` carries typed attributes,
`ctx["srcs"]` carries the raw glob patterns from the manifest,
`ctx["deps"]` carries provider records returned by analyzed dependencies, and
`ctx["build_dir"]` is the workspace-relative output directory.

The Rust side exposes only generic primitives; everything Apple-specific
(SDK names, triple format, `xcrun` resolution, file-extension filtering)
lives in `apple.star`:

- `host_arch()`, `host_os()` report the host CPU and OS.
- `host_which(name)` finds a binary on `PATH` and returns its absolute path.
- `host_command(argv)` runs an arbitrary command and returns its stdout.
- `glob(patterns)` expands a list of glob patterns against the active target's
  package directory and returns sorted, deduplicated, workspace-relative file
  paths.
- `declare_output(name)` reserves a workspace-relative path under the target's
  `build_dir`.
- `run_action(argv, inputs, outputs, env, toolchain_identity, identifier)`
  records one command to execute. `toolchain_identity` is folded into the
  action's input digest so a swap of Xcode (or whatever the prelude shelled
  out to) invalidates affected cache slots.

The `apple_library` impl composes its swiftc command line by calling
`host_which("xcrun")`, `host_command(...)` to resolve and version the active
`swiftc`, then rendering a triple and walking dep provider records to gather
`-I` search paths. The impl returns a provider dict that downstream rules read
from `ctx["deps"]`; for `apple_library` the provider carries `swiftmodule_dir`
(so consumers can add it to `-I`) and `archive` (so future linking rules can
reach the `.a`).

Script targets still execute through `once run` and the action cache. The
bundle and test rules will migrate to starlark impls alongside their
Xcode-driven actions in later implementations.

## Build Pipeline

A graph command opens a single `BuildSession` for the entire invocation.
The session owns:

- an `AnalysisEngine` that parses the prelude once and reuses both the
  rule schemas and a `HostCache` across every analyzed target;
- an `Arc<GraphTarget>` per target so spawned tasks share a cheap
  refcount handle instead of deep-cloning the graph row;
- the workspace path and cache provider for the lifetime of the build.

For a target that needs analysis, the session walks the impl-reachable
dependency closure, builds a per-target reverse-dependency map, and
schedules targets through a `tokio::task::JoinSet`. Sibling targets
without dependencies on each other compile in parallel, bounded only by
the runtime and the locks held inside the host cache.

A reader counter on each dependency lets the **last** consumer move the
producer's provider record out of the session map instead of cloning it,
so a large `JsonValue` tree only ever has one live owner. The
`HostCache` releases its lock before shelling out to `xcrun` and before
walking `PATH`, so a slow xcrun spawn on one task never blocks a sibling
task asking about a different binary.

The action input digest composes:

- `toolchain_identity` from the impl (the prelude folds in `swiftc`'s
  resolved path plus its version banner);
- the content digest of every input file passed to `run_action`;
- each direct dep's action digest, keyed by label.

So an Xcode swap, a source edit, or a transitive dep change each
invalidate exactly the affected cache slots without churning unrelated
ones.

## Agent Workflows (Planned)

The action cache and content-addressed storage already give every action a
stable digest and addressable outputs. A follow-up will expose this through an
MCP interface so coding agents can call `run_action` with a returned id, then
later query the cached outputs, logs, and provider records by that id without
re-running anything. Schema introspection, structured diagnostics, and the
graph queries above are designed to feed that surface.

## Prior Art and Tracking

The Apple rule set follows [RFC 0001: Once Build
Graph](https://github.com/tuist/once/blob/main/rfcs/0001-build-graph.md)
and adapts ideas from established Apple build tooling rather than
copying its surface:

- [Bazel rules_apple](https://github.com/bazelbuild/rules_apple), where
  application rules handle linking and bundling while Swift and
  Objective-C compilation live in dedicated language rules.
- [Bazel apple_binary](https://docs.bazel.build/versions/3.0.0/be/objective-c.html#apple_binary),
  which exposes Apple binary concepts such as platform type, minimum OS
  version, SDK frameworks, SDK dylibs, weak SDK frameworks, link
  options, and multi-architecture outputs.
- [Buck2 apple_library](https://buck2.build/docs/prelude/rules/apple/apple_library/),
  [apple_binary](https://buck2.build/docs/prelude/rules/apple/apple_binary/),
  and [apple_bundle](https://buck2.build/docs/prelude/rules/apple/apple_bundle/),
  which separate compile inputs, Apple toolchain selection, bundle
  assembly, resources, asset catalogs, Info.plist values, entitlements,
  provisioning profiles, and tests.

Once is not Buck-compatible, Bazel-compatible, or a drop-in replacement
for either tool; users and agents declare targets in `once.toml`,
built-in rule metadata lives in the Starlark prelude, and the graph is
intentionally inspectable first so agents and CLI users can ask what a
target can do before broad execution exists.
