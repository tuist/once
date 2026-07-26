---
prev: false
next: false
---

# Apple

Once can build libraries, frameworks, applications, and test bundles for
Apple platforms. This guide starts with one iOS application and a library it
depends on, then queries, builds, runs, and tests that same project.

## Prerequisites

Apple targets require a macOS host with the Apple developer tools used by
`xcrun`, `swiftc`, and `clang`. Verify that the Swift compiler is available:

```sh
xcrun --find swiftc
```

Running this guide's iOS application also requires an installed Simulator
runtime. Building a library does not require a running simulator.

## Declare the Application

Create `apps/Hello/once.toml` with a reusable library and an application that
depends on it:

```toml
[[target]]
name = "AppCore"
kind = "apple_library"
srcs = ["Sources/AppCore/**/*.swift"]

[target.attrs]
platform = "ios"
minimum_os = "17.0"

[[target]]
name = "Hello"
kind = "apple_application"
srcs = ["Sources/HelloApp/**/*.swift"]
deps = ["./AppCore"]

[target.attrs]
platform = "ios"
bundle_id = "dev.once.Hello"
minimum_os = "17.0"
families = ["iphone"]
```

`./AppCore` resolves in the same package. The dependency is typed:
`apple_application` accepts the linkable output produced by
`apple_library`.

The smallest useful source layout is:

```text
apps/Hello/
├── once.toml
└── Sources/
    ├── AppCore/
    └── HelloApp/
```

The application source needs an entry point, such as a Swift type marked with
`@main`. The library contains code imported by the application.

## Query Before Building

Inspect the exact targets declared above:

```sh
once query targets
once query capabilities apps/Hello/AppCore
once query capabilities apps/Hello/Hello
once query schema apple_application
```

`AppCore` exposes `build`. `Hello` exposes `build` and `run`. The schema query
shows the application attributes, accepted dependencies, capabilities, and
outputs without starting a compiler.

## Build and Run

Build the application bundle:

```sh
once build apps/Hello/Hello
```

Once builds `AppCore` first because `Hello` depends on it. Outputs are
materialized under `.once/out/<target>/`; the
[`apple_application` reference](/reference/prelude/apple_application) lists
the exact output groups.

Launch the application in an iOS simulator:

```sh
once run --visible apps/Hello/Hello
```

Once selects or boots a simulator, installs the application bundle, and
launches its bundle identifier. The launch runs on every invocation instead
of replaying from the action cache. Omit `--visible` when Simulator does not
need to be brought to the foreground.

macOS applications use the host application launcher. Launching directly on
an iPhone or iPad is not supported yet.

## Add a Test Target

Add a test target to the same `apps/Hello/once.toml`:

```toml
[[target]]
name = "AppCoreTests"
kind = "apple_test_bundle"
srcs = ["Tests/AppCoreTests/**/*.swift"]
deps = ["./AppCore"]

[target.attrs]
platform = "ios"
minimum_os = "17.0"
swift_testing = true
labels = ["unit"]
```

Place tests that use Swift Testing under `Tests/AppCoreTests/`, then inspect
and run the new capability:

```sh
once query capabilities apps/Hello/AppCoreTests
once test apps/Hello/AppCoreTests
```

Apple tests currently support macOS logic tests and iOS simulator bundles.
Application-hosted tests, custom destinations, test plans, and device runners
are not implemented. Non-empty unsupported attributes fail during graph
analysis instead of being ignored. See the
[`apple_test_bundle` reference](/reference/prelude/apple_test_bundle) before
adding those features.

## Choose Values by Configuration

Configurable attributes can use `select`. For example, a library shared by an
iOS and macOS target can choose the platform framework it links:

```toml
[target.attrs]
sdk_frameworks = { select = { ios = ["UIKit"], macos = ["AppKit"] } }
```

Apple configuration keys include platform names, architectures, `simulator`,
`device`, `mac_catalyst`, combined keys such as `ios:simulator`, and
`default`. When more than one branch matches, the most specific branch wins.

Attributes that determine the configuration, including `platform`,
`sdk_variant`, `archs`, and `mac_catalyst`, must remain literal. The target
kind schema identifies any other non-configurable attributes.

## Connect Native Dependencies

Apple targets can consume native outputs from other ecosystems through normal
`deps` entries:

- [`kotlin_apple_framework`](/reference/prelude/kotlin_apple_framework)
  produces a framework that an application or test can link and embed.
- [`rust_mobile_library`](/reference/prelude/rust_mobile_library) produces an
  Apple static library for an Apple consumer.

Add these only after the application builds on its own, then query the
consumer again to confirm that the dependency contract is satisfied.

## Supported Target Kinds and Limitations

Use the target kind reference for the contract that matches the artifact:

- [`apple_library`](/reference/prelude/apple_library)
- [`apple_framework`](/reference/prelude/apple_framework)
- [`apple_application`](/reference/prelude/apple_application)
- [`apple_test_bundle`](/reference/prelude/apple_test_bundle)
- [`swift_macro`](/reference/prelude/swift_macro)

Application resource bundles, asset catalogs, custom property-list templates,
entitlements, provisioning profiles, signing identities, and non-ad-hoc
signing are accepted by the schema but are not supported yet. Using a non-empty
value for one of these attributes fails validation before the build starts.

## Next

Continue with [Memory](/guide/memory/) once the application builds and tests.
It shows how Once records durable context about graph work. If the application
also contains Android or Rust code, use [Ecosystems](/guide/graph/ecosystems)
to choose the next independent integration.
