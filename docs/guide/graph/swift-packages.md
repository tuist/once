---
prev: false
next: false
---

# Swift Packages

Once can import a locked Swift Package Manager graph and expose selected static
products to Apple libraries, frameworks, applications, and tests. Swift Package
Manager remains authoritative for package manifests, version selection, and
`Package.resolved`; Once makes the locked result queryable and cacheable.

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

## Declare the Locked Package Graph

Keep package declarations in `Package.swift` and exact remote selections in
`Package.resolved`. A checked-in dependency graph avoids running package
inspection while Once loads the graph.

```toml
[[target]]
name = "Packages"
kind = "swift_package_dependencies"
srcs = [
  "Package.swift",
  "Package.resolved",
  "swiftpm-dependencies.json",
  "Sources/**/*.swift",
  "Vendor/Greeting/**/*.swift",
  "Vendor/Greeting/Package.swift",
]

[target.attrs]
package_path = "."
resolved_file = "Package.resolved"
resolver_inputs = [
  "Package.swift",
  "Package.resolved",
  "swiftpm-dependencies.json",
  "Vendor/Greeting/Package.swift",
]
graph_file = "swiftpm-dependencies.json"
products = ["Root", "Greeting"]
platform = "macos"
minimum_os = "13.0"
```

Every name in `products` must be a static library product. Include the root
facade and every transitive archive needed by downstream links. Once does not
guess archive names for automatic library products.

`resolved_file` and `graph_file` are relative to `package_path`. Their defaults
therefore follow a package moved into a workspace subdirectory.

The bundled starter uses this layout:

```text
Package.swift
Package.resolved
swiftpm-dependencies.json
Sources/Root/Root.swift
Vendor/Greeting/Package.swift
Vendor/Greeting/Sources/Greeting/Greeting.swift
```

`Root` imports `Greeting` and exposes one function to first-party consumers.
The local package keeps the starter network-independent while exercising the
same package graph and product linking boundary as a vendored remote package.

## Connect a First-Party Consumer

Add the dependency-set target to an ordinary Apple target:

```toml
[[target]]
name = "Consumer"
kind = "apple_application"
srcs = ["Consumer/**/*.swift"]
deps = ["./Packages"]

[target.attrs]
platform = "macos"
bundle_id = "dev.once.Consumer"
minimum_os = "13.0"
```

The consumer can import the root product normally:

```swift
import Root

@main
struct Consumer {
    static func main() {
        print(greet())
    }
}
```

The `deps` edge carries the package module directory, selected archives,
frameworks, dynamic libraries, and linker options into the Apple target.

## Query and Build the Expanded Graph

Inspect the synthetic pins and the two buildable targets before execution:

```sh
once query targets --kind swift_package_pin
once query capabilities Packages
once query capabilities Consumer
once build Consumer
./.once/out/Consumer/Consumer.app/Consumer
```

The starter creates a `swiftpm-greeting-local` pin, performs one locked package
build for `Root` and `Greeting`, and links both products into `Consumer`. Running
the built application prints `Hello from a locked local package`.

Synthetic `swift_package_pin` targets carry identity, revision, version,
checksum, location, and package-to-package edges. They have no compile action.
The `swift_package_dependencies` owner performs the locked native package build
and emits the provider used by the local Apple consumer.

## Update or Vendor Packages Explicitly

Update package state outside an ordinary Once build:

```sh
xcrun swift package resolve
xcrun swift package show-dependencies --format json > swiftpm-dependencies.raw.json
# Add the Once provenance fields, then save the result as swiftpm-dependencies.json.
once query targets --kind swift_package_pin
once build Consumer
```

The Swift Package Manager command emits only its raw package graph. Review
that graph and `Package.resolved` together, then add the required provenance
fields before using it as `swiftpm-dependencies.json`. The
[JavaScript Object Notation](https://www.json.org/json-en.html) graph snapshot
must contain every locked pin, or graph loading rejects
it as stale. Add `once_manifest` with the exact `Package.swift` text and
`once_resolved` with the exact `Package.resolved` text. This makes manifest,
lockfile, and local dependency changes reject a stale graph. Add `once_inputs`
as a map containing every exact resolver input except the graph snapshot
itself. Include each workspace-local package manifest in `resolver_inputs` so
its dependency edges participate in that binding. Any graph node without a
lock entry must resolve to a workspace-local path. For
network-independent remote builds, set `vendor_path` to a complete Swift
Package Manager scratch tree containing the matching checkouts,
repositories, and workspace state. Once copies that tree into target-local
output before building, so the checked-in vendor state is not mutated.

Without a graph snapshot, graph loading runs locked package inspection in a
disposable `.once` scratch directory. Remote packages require an explicit
`allow_network = true` for that live inspection. A configured `vendor_path`
requires a checked-in graph snapshot, which prevents analysis from mutating the
vendored tree. Automatic version selection remains disabled. Once does not
independently sandbox Swift Package Manager network access during a build, so an
incomplete vendored tree can still cause native package-manager access. By
default, Once uses the Swift Package Manager executable paired with the Apple
compiler selected through Xcode, so package modules and first-party consumers
cannot silently compile with different Swift versions.

Locked package identities, versions, revisions, and checksums participate in
the native build action identity even when `Package.resolved` is kept only in
`resolver_inputs`. Updating a pin therefore invalidates the package build
without requiring the lock file to be duplicated in `srcs`.

## Current Boundary

The adapter currently exposes explicit static products for one Apple platform
and architecture per target. Resource bundles, binary artifacts, and build-tool
plugins need typed provider fields before Once can safely embed or execute them.
Use a first-party static package facade or a script for those packages until
their artifacts are represented explicitly.

See [`swift_package_dependencies`](/reference/prelude/swift_package_dependencies)
for every attribute and provider field, and
[`swift_package_pin`](/reference/prelude/swift_package_pin) for the synthetic
locked identity shape.
