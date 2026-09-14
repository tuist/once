---
prev: false
next: false
---

# Swift Packages

Once reads `Package.swift` and derives first-party package targets as Once
targets. Swift Package Manager remains authoritative for manifests, version
selection, and `Package.resolved`; Once uses that information to build and
cache the targets it derives.

## Start From a Native Package

Once recognizes `Package.swift` automatically and lowers first-party package
targets into the existing Apple target kinds. This works without `once.toml`:

```sh
once query workspace
once query targets
once build --ui
once test
```

The generated `swift_package_workspace` seed reads the package manifest and
derives first-party libraries, executables, macros, binary targets, and tests.
Discovery skips generated `.build` and `.swiftpm` directories and does not
write `once.toml`. `once build` selects the package workspace when it is the
only discovered build root. `--ui` opens the Runs interface so the first
compile is visible target by target. `once test` runs first-party test bundles
and excludes test bundles that belong only to resolved packages. Use `once
test --all` when you intentionally want the complete resolved test graph.

Local path dependencies within the workspace are followed transitively,
including sibling packages and shared dependencies reached through several
products. An entirely local graph does not need `Package.resolved` and does not
run dependency resolution.
If a local dependency introduces a remote package, Swift Package Manager still
resolves that package through the root package's lockfile.

## Keep Swift Package Manager Commands

For explicit module builds, add `explicit_modules = true` to your
`swift_package_workspace` seed's attributes. Optional
`dependency_check = "error"` checks source imports against declared package
dependencies. Once propagates these settings through the resolved package
graph; `Package.swift` remains the source of target and dependency declarations.
See [explicit Apple modules](/guide/graph/apple#explicit-modules-and-dependency-checks)
for requirements and current limits.

Once can sit behind the `swift` command for one native [Swift Package
Manager](https://www.swift.org/documentation/package-manager/) package. Add
this mise wrapper, then run `mise reshim` and activate mise in the shell that
starts the build:

```toml
[wrappers.swift]
command = "once"
args = ["swift", "--"]
```

Once routes the default debug build and test forms into the native graph:

```sh
swift build
swift test
```

`swift build` builds the package seed and its resolved products. `swift test`
runs each first-party test bundle once through Once, excluding test targets
that belong to resolved dependencies. `-q` or `--quiet`,
`-c debug` or `--configuration debug`, and `--package-path .` are also
supported.

For troubleshooting without mise, call the compatibility surface directly and
put the separator before the Swift Package Manager arguments:

```sh
once swift -- test
```

Every other request, including release builds, filters, package subcommands,
plugins, and a package path outside the current directory, invokes the system
`swift` executable with its original arguments and exit status. This preserves
Swift Package Manager behavior until the request has an exact Once equivalent.

For locked source-control dependencies, native package lowering materializes
the pinned sources during graph loading, then compiles them directly through
Once's Apple target kinds. See
[`swift_package_workspace`](/reference/prelude/swift_package_workspace) for
its complete contract.

When the Xcode workspace resolver lowers package sources into Apple targets,
compiler language and access-control flags follow the manifest tools version.
Once adds the semantic package name only for tools version 5.9 or newer, where
package-scoped access is part of the manifest contract. Older manifests remain
isolated by module.

## Prerequisites

Install Xcode and its command-line tools, then verify the selected Swift
toolchain:

```sh
xcrun --find swift
xcrun swift --version
```

Once selects the Swift compiler through `xcrun`. A Swiftly `.swift-version`
file does not change that selection. If a package requires a separately
installed Swift toolchain, select its bundle identifier for the build:

```sh
TOOLCHAINS=your.toolchain.bundle.identifier once build
```

Check the selection with `TOOLCHAINS=your.toolchain.bundle.identifier xcrun
swift --version`. The selected compiler must support the package's
`swift-tools-version`.

A Swift toolchain installed beside Xcode contributes the compiler and the
macros it loads, while Xcode still contributes the linker and the software
development kit. Once resolves each of those separately, so builds that mix
them link and expand macros against matching tools. Swift Testing follows the
same rule: when the selected compiler ships its own copy, tests compile and
link against that one rather than the version Xcode publishes.

Start with the [Apple guide](/guide/graph/apple) if a first-party Apple target
does not build yet. Package integration is easier to diagnose after the local
compiler, software development kit, linker, and code-signing path work.

## Packages With Remote Dependencies

Native package lowering uses `Package.resolved` to materialize each
source-control dependency at its pinned revision. When a package declares
dependencies but does not commit the lockfile, Once asks Swift Package Manager
to resolve them and uses the generated lockfile. It then derives and compiles
the dependency targets directly, so Swift Package Manager does not build a
separate dependency graph. Registry dependencies are not supported by this
native path yet.

The ordinary workflow stays the same:

```sh
once query targets
once build SwiftPackage_MyPackage_MyLibrary
```

There is no Once initialization step. A first build can access the network when
Swift Package Manager must create `Package.resolved` or Once must materialize a
pinned source dependency.

Once preserves package traits requested by dependencies, including traits that
enable other traits and conditional requests. Code guarded by a dependency's
opt-in trait is compiled with that trait enabled. Explicit empty trait lists
disable defaults for that dependency declaration; requests from other packages
are still combined according to Swift Package Manager's rules.
