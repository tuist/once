# Native graph acceptance fixtures

These are original, deliberately tiny programs, not copies of application
implementation code. They retain graph patterns observed in:

- [Vapor 4.122.1](https://github.com/vapor/vapor/tree/6f8091db7f159f966e028fbbc928cdcfa8597d59).
- [NetNewsWire](https://github.com/Ranchero-Software/NetNewsWire/tree/dc74019c2434acf4a9a7826e709257a4a2541361).

The Vapor shape keeps three library products, seven first-party targets,
executable and test-only branches, a C module, shared product dependencies,
conditional dependencies, excluded sources, and a resource with spaces in its
name. A header-only system library mirrors the validation support module in
Vapor's resolved dependencies, including its host-tool edges. Its networking
and logging packages are local stand-ins for upstream
remote dependencies. The networking product deliberately exports two modules
to test product expansion independently of module names.

The NetNewsWire shape keeps eight native application, extension, and hosted-test
targets across macOS and iPhone destinations. It retains layered configuration
files, shared source membership, package products without explicit package
references, sibling package dependencies, a shared dependency diamond, and an
Objective-C module. Package tests and resources stay in the graph but outside
the requested application build. A tiny C archive is packaged as a binary
framework for macOS and the iPhone simulator during fixture setup, exercising
destination-specific imports without downloading vendor binaries.

Both fixtures are offline, use real native manifests, and need no `once.toml`.
Tests compile only the requested small macOS closure. They inspect the iPhone
graph without starting a simulator or compiling that application. Implicit
compiler modules keep this graph-focused suite independent of the more
expensive explicit-module acceptance tests. No fixture performs network
fetches, builds upstream applications, or recompiles the Once executable.

Run after the normal release build:

```sh
mise exec -- shellspec spec/native_apple_graphs_spec.sh
```

The two serial workflows took 104 seconds on the development host, including
fixture preparation, graph queries, repeated builds, and cache checks. The
nine accompanying compiler-free regression checks took 1.3 seconds. Upstream
repository downloads and full application builds are not part of this suite.

Assertions cover graph closure, destination-correct edges, first-party test
selection metadata, native source and configuration changes, selective builds,
warm cache hits, restoration after removing outputs, and invalidation from
both native headers and transitive package sources. Binary archive downloads,
application behavior, extension packaging, and upstream test correctness are
not covered by these reduced fixtures.
