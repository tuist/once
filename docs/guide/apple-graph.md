# Apple Graph

Once is starting its build graph with Apple targets. The first implementation
models target declarations, rule schemas, capabilities, and local build, run,
and test commands. It does not invoke Xcode yet.

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
names or macro model. Users and agents declare graph targets in `once.toml`.
Built-in Apple rule metadata is defined in Once's Starlark prelude, then lowered
into typed Rust graph schemas. The graph is intentionally inspectable first, so
agents and CLI users can ask what a target can do before broad execution exists.

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

Script targets still execute through `once run` and the action cache. Apple
target execution will move from these local materialization actions to Xcode
toolchain-backed configured graph snapshots and concrete compile, link, bundle,
sign, install, launch, and XCTest actions in later implementations.
