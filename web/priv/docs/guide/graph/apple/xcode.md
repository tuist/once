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

Once honors `SWIFT_ENABLE_EXPLICIT_MODULES = YES` on native targets. You can
also set `explicit_modules = true` on an `xcode_workspace` seed to enable the
mode across its Swift targets and resolved packages. Optional
`dependency_check = "error"` rejects undeclared workspace imports. Import edges
inferred by Once do not count as declarations for this check. See
[explicit modules](/guide/graph/apple#explicit-modules-and-dependency-checks)
for requirements and current limits.

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
once query workspace
once query targets
```

Once detects the project, supplies an ephemeral `xcode_workspace` seed, and
resolves the graph. Build and test commands work against that graph
immediately, with no `once.toml` in the repository.

For the shortest end-to-end trial, build the discovered project and watch the
live action graph in the Runs interface:

```sh
once build --ui
once test
```

No scheme or Once target name is needed when the directory contains one
discovered Xcode project. If Once discovers several project roots, use the
identifiers from `once query targets` to choose one explicitly.

Discovery does not write `once.toml`. The `.xcodeproj` stays authoritative for
everything the seed resolves.

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

Building without a target selects the one discovered project seed. Application
projects build their application roots; library-only projects build their
non-test products:

```sh
once build --ui
```

Omit `--ui` for ordinary terminal-only output.

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

Run `once test` with no target to select the first-party test bundles rooted in
the discovered project. `once test --all` includes tests from the complete
resolved graph.

See [Testing and Scheduling](/guide/graph/testing) for selection and reporting,
and that target kind's limitations for the test shapes that are supported.

## Keep `xcodebuild` Commands

Once can also sit behind the familiar `xcodebuild` command when a project uses
[mise](https://mise.jdx.dev/) command wrappers. Add this to the project's
`mise.toml`, then run `mise reshim` and activate mise in the shell that starts
the build:

```toml
[wrappers.xcodebuild]
command = "once"
args = ["xcodebuild", "--"]
```

The wrapper preserves the original arguments. The separator belongs to Once and
is not passed to Xcode. Once uses the graph for a build
when it can prove the request is equivalent: one resolved Once Xcode workspace
seed for a discovered project, a scheme that exactly matches one reachable Once
build target, an explicit `-configuration Debug` argument, and the default or
explicit `build` action. An explicit `-project` value must name that discovered
project. For example:

```sh
xcodebuild -project Client.xcodeproj -scheme Client -configuration Debug build
```

For troubleshooting without mise, call the compatibility surface directly and
put the separator before the Xcode arguments:

```sh
once xcodebuild -- -project Client.xcodeproj -scheme Client -configuration Debug build
```

Every other request invokes the system `xcodebuild` with the same arguments and
exit status. This includes tests, archives, exports, destinations, package
resolution, custom build settings, workspaces, missing or other configurations,
help, and version queries. The fallback lets editors, continuous integration,
and coding agents keep their existing command vocabulary while Once takes over
only the forms it can model accurately.

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
- Shell script build phases, including phases without declared outputs.
  Preparation and source generators run before compilation. Scripts that
  feed resource phases run after linking and before packaging. Product scripts
  run after assembly and before final signing.
- Core Data models and Intents definitions, whose generated sources are
  compiled with the target.
- Swift package dependencies, both local packages in the repository and remote
  packages, lowered into Apple library targets.

A project containing both macOS and iPhone targets gets separate package
targets for each destination. Each application and extension depends on the
matching package graph, including platform-conditional dependencies and binary
framework slices. Once shares manifest parsing across destinations. Local
products that Xcode references without a package reference are discovered from
the repository's package manifests, including their transitive local dependencies.

## Script Build Phases

Once runs imported scripts using their declared shell and selected build
configuration. It expands project and target settings, including custom
settings from configuration files, into the script environment. Directory
settings such as `SRCROOT`, `PROJECT_DIR`, `TARGET_BUILD_DIR`,
`BUILT_PRODUCTS_DIR`, `DERIVED_FILE_DIR`, and `TARGET_TEMP_DIR` point to Once's
workspace and products, not an Xcode derived-data directory. Product settings
such as `EXECUTABLE_PATH`, `INFOPLIST_PATH`, and
`UNLOCALIZED_RESOURCES_FOLDER_PATH` describe the bundle Once actually creates.
Configuration-specific Once output directories are reflected at execution.

Input and output paths become graph inputs and outputs. File-list contents
are expanded for dependency tracking, while the script receives the separate
`SCRIPT_INPUT_FILE_*`, `SCRIPT_OUTPUT_FILE_*`, and `SCRIPT_*_FILE_LIST_*`
variables and counts described in [Apple's script documentation](https://developer.apple.com/documentation/xcode/running-custom-scripts-during-a-build).
Dependency products are staged alongside the current product for scripts
that use `BUILT_PRODUCTS_DIR`.

Scripts with workspace-contained declared inputs and outputs can be cached.
Native shell phases execute in the workspace, not an isolated copied-input
sandbox, so absolute build-setting paths and in-place product edits refer to
the same files. Cache correctness depends on complete declarations.
Scripts with no complete declaration, external inputs, `alwaysOutOfDate`, or
dependency analysis disabled run each time. Their target reports cache bypass,
although individual compile actions can still hit the cache. Untracked
post-build scripts capture the resulting bundle, including newly created
resources, before final signing.
Files deleted by a product script stay deleted in the resulting bundle.
Installation-only phases do not run during `once build`.

## Limitations

Once compiles the project itself rather than delegating to `xcodebuild`, so
anything outside the project's own description has to be in place beforehand.
Once does not install dependencies or bootstrap external native toolchains.
Run tools such as `pod install` first so their generated projects, scripts,
and resources exist before Once imports the graph. Script phases in supported
imported targets then run through Once.
Unresolved dependency-generated file-list settings do not prevent graph import,
but building the affected target reports the missing setup. A resolved file-list
path that does not exist is an import error.

Script ordering uses preparation, pre-package, and post-package stages. Projects
that interleave repeated copy or resource phases with scripts may require
explicit graph declarations to preserve that finer-grained ordering.

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
