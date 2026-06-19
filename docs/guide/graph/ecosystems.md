# Ecosystems

Ecosystems are Once's built-in target kind sets for a programming
language, platform, or build domain. Apple, Android, and Rust are
ecosystems because each one needs its own vocabulary: source files,
native tools, dependency metadata, generated outputs, test runners, and
runtime behavior.

The lower-level [target kind reference](/reference/prelude/) lists the
generated attribute, provider, and capability tables. In the guide, the
useful product model is ecosystem: a supported build world that Once can
describe, query, run, cache, and explain.

## What Changes

Adopting an ecosystem means choosing Once's graph model for that part of
the project. Once is no longer just shelling out to the native tool and
hoping the result can be understood later. It asks you to declare the
important units, dependencies, attributes, and capabilities so the graph
can answer questions before work runs.

That is the tradeoff. Once should make a smaller version of this bet
than a full build-system migration, but it is still a bet: the ecosystem
integration becomes product surface.

## The Caveats

The main caveat is feature coverage. Native ecosystems evolve quickly,
and a Once ecosystem may lag behind native package managers, compilers,
test runners, or IDEs. A target kind should expose the common path
well, then make unsupported features obvious through structured
diagnostics instead of failing deep inside a generated command.

External dependencies are the hardest part to get right. Dependency
resolution, repository materialization, version selection, vendoring,
and integration with language-specific package managers are separate
concerns. Once has to decide which system owns resolution for each
ecosystem. Rust may start from Cargo metadata, Android may start from
Gradle or Maven coordinates, and Apple may start from source targets or
SwiftPM packages. Those bridges will not support every native feature on
day one.

Toolchains are another commitment. Different projects source compilers,
SDKs, linkers, and flags in different ways. Once ecosystems should avoid
assuming every machine has the same tools on `PATH`. They need explicit
toolchain discovery, diagnostics, and eventually project-specific
toolchain configuration.

IDE integration is a product feature, not a side effect. If an ecosystem
replaces part of the native build graph, editors may need generated
project files, language-server configuration, or query APIs to recover
the experience developers expect. At the same time, IDE parity is less
absolute in a world where coding harnesses can query the graph, inspect
schemas, run focused checks, and read memory through MCP. Once should
preserve the human editing loop, but it should not treat a native IDE
clone as the only path to a good developer experience.

Dynamic behavior needs boundaries. If an ecosystem needs to read
generated metadata before deciding what to do next, Once should model
that as a structured mechanism rather than letting arbitrary scripts
reshape the graph invisibly.

## How To Adopt One

Start with the ecosystem target kinds when you want the graph to know
about a domain, not merely run a command in that domain. Use scripts
when the integration is exploratory, when the native tool is still the
source of truth, or when a feature has not been modeled yet.

For a production ecosystem, expect to answer these questions:

- Which native concepts become targets?
- Which attributes are stable enough to declare?
- Which dependency manager owns third-party resolution?
- Which outputs are reusable build artifacts?
- Which commands are runtime effects that should not replay from cache?
- Which IDE or language-server affordances must be preserved?
- Which unsupported native features need explicit diagnostics?

That boundary is the point. Once ecosystems should make the build graph
more inspectable and agent-friendly, but they should not hide the cost of
forking part of a native build world into Once's model.
