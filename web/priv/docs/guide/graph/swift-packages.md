---
prev: false
next: false
---

# Swift Packages

Once reads `Package.swift` and derives first-party package targets as Once
targets. Swift Package Manager remains authoritative for manifests, version
selection, and `Package.resolved`; Once uses that information to build and
cache the targets it derives.

## Start From A Native Package

Once recognizes `Package.swift` automatically and lowers first-party package
targets into the existing Apple target kinds. This works without `once.toml`:

```sh
once native list
once native show swift_package
once query targets
```

If a repository contains several package roots, select the workspace-relative
root path reported by discovery:

```sh
once native show swift_package --path modules/service
```

The generated `swift_package_workspace` seed reads the package manifest and
derives first-party libraries, executables, macros, binary targets, and tests.
Discovery skips generated `.build` and `.swiftpm` directories. Store the seed
only when it should be reviewed in `once.toml`:

```sh
once native init swift_package
```

## Keep Swift Package Manager Commands

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

Start with the [Apple guide](/guide/graph/apple) if a first-party Apple target
does not build yet. Package integration is easier to diagnose after the local
compiler, software development kit, linker, and code-signing path work.

## Packages With Remote Dependencies

Commit `Package.resolved` with `Package.swift`. Native package lowering uses
the lockfile to materialize each source-control dependency at its pinned
revision. It then derives and compiles the dependency targets directly, so
Swift Package Manager does not build a separate dependency graph. Registry
dependencies are not supported by this native path yet.

The ordinary workflow stays the same:

```sh
once native show swift_package
once build SwiftPackage_MyPackage_MyLibrary
```

There is no initialization step or network setting for the native workflow.
