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

By default, native package lowering does not fetch remote dependencies during
graph loading. See [`swift_package_workspace`](/reference/prelude/swift_package_workspace)
for its complete contract.

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

Commit `Package.resolved` with `Package.swift`. Native discovery reads those
files locally and never downloads packages. When a build first needs a locked
remote product, Once fetches and builds it as a dependency action. Later builds
reuse that action from the cache while the manifest, lock file, and Swift
toolchain remain unchanged.

The ordinary workflow stays the same:

```sh
once native show swift_package
once build SwiftPackage_MyPackage_MyLibrary
```

There is no initialization step or network setting for the native workflow.
