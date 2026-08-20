---
prev: false
next: false
---

# Xcode Projects

Once can read an Xcode project or workspace you already have and derive a typed
graph from it. You do not migrate the project, generate a new one, or restate
its targets in `once.toml`. The `.xcodeproj` stays the source of truth for
targets, build settings, and file membership, and Once compiles what it finds
there.

This is the fastest way to try Once against real code. A project at the
repository root needs no manifest at all: run one query and see your
application, frameworks, libraries, and test bundles as Once targets.

## Prerequisites

Reading an Xcode project requires a macOS host with Xcode and its command-line
tools. The resolver uses `plutil` to convert `project.pbxproj` and `xcrun` to
locate compilers and software development kits:

```sh
xcrun --find swiftc
plutil -help
```

If the project depends on a package manager or code generator that Once does
not run, complete that step first. Once reads the project as it stands on disk,
so run `pod install`, fetch vendored binaries, or execute a bootstrap script
before pointing Once at the result.

## Try It Without a Manifest

An Xcode project checked in beside the repository root is a recognized native
project, so there is nothing to write. From the directory holding the
`.xcodeproj`:

```sh
once native list
once query targets
```

Once detects the project, supplies an ephemeral `xcode_workspace` seed, and
resolves the graph. Build and test commands work against that graph
immediately, with no `once.toml` in the repository.

Persist the seed only when you want it under version control:

```sh
once native init xcode
```

That writes the seed target and nothing else. The `.xcodeproj` stays
authoritative for everything the seed resolves.

## Declare the Seed Explicitly

Write the target yourself when the project is not at the repository root, when
a directory holds more than one project, or when you want to pin a
configuration:

```toml
[[target]]
name = "App"
kind = "xcode_workspace"
srcs = ["App.xcodeproj/project.pbxproj"]

[target.attrs]
project = "App.xcodeproj"
```

The `project` attribute can be omitted when the package contains exactly one
`*.xcodeproj`. Declaring it is clearer, and it is required when a directory
holds more than one project. Declare the target in the workspace root package
and point `project` at the nested path when the project lives in a
subdirectory.

`srcs` supplies the resolver's inputs, which Once reads as text and hashes so
that editing the project re-resolves the graph. Keep them to `project.pbxproj`
and other text files such as `.xcconfig` and `.xcscheme`. A glob that pulls in
binary assets fails resolution, because resolver inputs must be valid
[Unicode Transformation Format, 8-bit (UTF-8)](https://www.unicode.org/faq/utf_bom.html#UTF8)
text.

## See the Derived Graph

Ask for the targets before building anything:

```sh
once query targets
```

The output contains the seed target plus one target per native target in the
project. A browser-style application project resolves into something like
this:

```text
targets:
  xcode (xcode_workspace) [build]
    tool xcode: plutil, xcrun
  Client (apple_application) [build, run]
  BrowserKit (apple_library) [build]
  Tabs (apple_library) [build]
  Bookmarks (apple_library) [build]
  ResourceBundle (apple_resource_bundle) [build]
  ClientTests (apple_test_bundle) [build, test]
  XCFramework_RustComponents.xcframework (apple_xcframework_import) [build]
```

Each native target is lowered into the Apple target kind that matches its
product type:

| Xcode product | Once target kind |
| --- | --- |
| Application, application extension | [`apple_application`](/reference/prelude/apple_application) |
| Framework | [`apple_framework`](/reference/prelude/apple_framework) |
| Static library, static framework | [`apple_library`](/reference/prelude/apple_library) |
| Unit and interface test bundle | [`apple_test_bundle`](/reference/prelude/apple_test_bundle) |
| Resource-only bundle | [`apple_resource_bundle`](/reference/prelude/apple_resource_bundle) |
| Referenced `.xcframework` | [`apple_xcframework_import`](/reference/prelude/apple_xcframework_import) |

Because the result is an ordinary typed graph, every other command works on it.
Inspect one lowered target and the contract it satisfies:

```sh
once query target Client
once query capabilities BrowserKit
```

## Build and Test

Building the seed builds every target the project resolved to:

```sh
once build xcode
```

Building one lowered target builds only that target and its dependencies:

```sh
once build BrowserKit
```

Run the same command twice. The second run is a cache hit, because the lowered
targets are ordinary Once targets with content-addressed actions.

Test bundles lower to [`apple_test_bundle`](/reference/prelude/apple_test_bundle)
targets carrying the `test` capability, so they schedule like any other Once
test target:

```sh
once test ClientTests
```

See [Testing and Scheduling](/guide/graph/testing) for selection and reporting,
and that target kind's limitations for the test shapes that are supported.

## Work From a Workspace

Point `project` at an `.xcworkspace` to resolve every project the workspace
references:

```toml
[[target]]
name = "App"
kind = "xcode_workspace"
srcs = ["App.xcworkspace/contents.xcworkspacedata", "App.xcodeproj/project.pbxproj"]

[target.attrs]
project = "App.xcworkspace"
```

Once enumerates each referenced `.xcodeproj`, lowers all of their native
targets, and merges them into one graph, so a dependency that crosses a project
boundary is wired. A workspace that references a project which is not on disk
yet, such as a generated project or `Pods.xcodeproj` before `pod install`, has
that project skipped instead of failing the whole graph. A project you
configure directly is always attempted, so a typo surfaces as a clear error.

## Choose Settings

The seed's attributes select which slice of the project is read:

```toml
[target.attrs]
project = "App.xcodeproj"
configuration = "Release"
sdk_variant = "device"
```

- `configuration` picks the Xcode build configuration whose settings drive
  lowering. It defaults to `Debug`.
- `sdk_variant` selects `simulator` or `device` for lowered targets on
  platforms other than macOS.
- `xcode_developer_dir` pins a `DEVELOPER_DIR` and folds it into the lowered
  targets' cache keys.
- `resolver_inputs` overrides the text globs read during resolution when they
  should differ from `srcs`.

## What the Resolver Reads

Understanding what is honored helps explain a target that resolves differently
than expected:

- Layered build settings from the project, the target, and any `.xcconfig`
  files, including `#include` directives, `$(inherited)`, variable expansion,
  and conditional keys such as `SWIFT_FLAGS[sdk=iphonesimulator*]`.
- File references from classic build phases and from Xcode 16 file-system
  synchronized groups, including per-target membership exceptions and
  exclusion patterns.
- Schemes, which identify testable targets so a test bundle is wired to its
  host application.
- Shell script build phases, replayed as prebuild actions whose declared
  outputs feed the same target's compile.
- Core Data models and Intents definitions, whose generated sources are
  compiled with the target.
- Swift package dependencies, both local packages in the repository and remote
  packages, lowered into Apple library targets.

## Limitations

Once compiles the project itself rather than delegating to `xcodebuild`, so
anything outside the project's own description has to be in place beforehand.
Dependency managers that integrate through their own build steps, notably
CocoaPods resource bundles and script phases, and native toolchains that a
repository bootstraps separately, are not reproduced by Once. Resolve them with
their own tooling first, then build.

App extensions and embedded watch apps compile as application bundles. The
`.appex` wrapper and its extension-point metadata are not modeled yet, so their
sources compile and cache but the embedded bundle layout is not reproduced.

Targets with no compilable sources, such as a script-only extension, are
skipped rather than lowered. Dependency edges to targets that were not emitted
are dropped so the remaining graph stays loadable.

## Next

Read [Apple](/guide/graph/apple) for the target kinds the project lowers into
and how to declare them directly, and
[Swift Packages](/guide/graph/swift-packages) for how package dependencies are
resolved.
