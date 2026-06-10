# Apple Graph

Once is starting its build graph with Apple targets, following
[RFC 0001: Once Build Graph](https://github.com/tuist/once/blob/main/rfcs/0001-build-graph.md).
The first implementation models target declarations, rule schemas,
capabilities, and local build, run, and test commands. `apple_library` is
driven by a starlark rule implementation in the prelude and invokes a real
`xcrun`-backed `swiftc` compile on macOS hosts. The other Apple rule kinds
(`apple_framework`, `apple_application`, `apple_test_bundle`) keep their
placeholder shell scripts until their toolchain plumbing lands.

The model is informed by:

- [Bazel rules_apple](https://github.com/bazelbuild/rules_apple), where
  application rules handle linking and bundling while Swift and Objective-C
  compilation live in language rules.
- [Bazel apple_binary](https://docs.bazel.build/versions/3.0.0/be/objective-c.html#apple_binary),
  which exposes Apple binary concepts such as platform type, minimum OS
  version, SDK frameworks, SDK dylibs, weak SDK frameworks, link options, and
  multi-architecture outputs.
- [Buck2 apple_library](https://buck2.build/docs/prelude/rules/apple/apple_library/),
  [apple_binary](https://buck2.build/docs/prelude/rules/apple/apple_binary/),
  and [apple_bundle](https://buck2.build/docs/prelude/rules/apple/apple_bundle/),
  which separate compile inputs, Apple toolchain selection, bundle assembly,
  resources, asset catalogs, Info.plist values, entitlements, provisioning
  profiles, and tests.

Once adapts those concepts into typed graph data instead of copying the rule
names or macro model. It is not Buck-compatible, Bazel-compatible, or a drop-in
replacement for either tool. Users and agents declare graph targets in
`once.toml`. Built-in Apple rule metadata is defined in Once's Starlark prelude,
then lowered into typed Rust graph schemas. The graph is intentionally
inspectable first, so agents and CLI users can ask what a target can do before
broad execution exists.

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

For `apple_library`, `build` invokes `xcrun --sdk <sdk> swiftc -emit-library
-static -emit-module` and produces a real static archive, swiftmodule, and
swiftdoc. When the target depends on other `apple_library` targets, Once builds
them first and forwards each dep's swiftmodule directory through a `-I` search
path so `import` statements resolve. The action cache key composes the active
swiftc identity, source content digests, and the dep action digests, so a
change to a dep's source rebuilds dependents on next invocation.

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

## Agent Workflows (Planned)

The action cache and content-addressed storage already give every action a
stable digest and addressable outputs. A follow-up will expose this through an
MCP interface so coding agents can call `run_action` with a returned id, then
later query the cached outputs, logs, and provider records by that id without
re-running anything. Schema introspection, structured diagnostics, and the
graph queries above are designed to feed that surface.
