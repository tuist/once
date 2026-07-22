# Request for Comments 0005: Locked Dependency Adapters

## Summary

Once should integrate external ecosystems by importing the native package
manager's locked result into the typed build graph. The native manager remains
authoritative for manifests, registry policy, version selection, and lockfile
serialization. A Starlark resolver turns that result into ordinary synthetic
targets that Once can validate, query, cache, and schedule.

The first adapters cover Cargo for Rust, Mix and Hex for Elixir, Swift Package
Manager, and Zig packages. They share one graph expansion contract and do not
add ecosystem branches to the Rust implementation.

## Decision

A target kind may declare `resolver = function`. During graph loading, Once:

1. Reads the resolver target's declared text sources.
2. Evaluates the resolver with typed attributes, source patterns, file
   contents, and generic host discovery primitives.
3. Validates a returned set of synthetic target records.
4. Normalizes ordinary and named dependency edges.
5. Merges resolver-owned attributes into the owner target.
6. Adds explicit resolved roots as owner dependencies.
7. Validates and schedules the expanded graph through the normal target kind
   and provider contracts.

Resolvers return graph data only. They cannot declare actions or outputs.
Synthetic targets use the same `name`, `kind`, `deps`, named dependencies,
sources, and typed attributes as manifest targets. Duplicate labels and invalid
references fail graph loading.

## Why Native Resolution Stays Authoritative

Package resolution includes more than version ranges. An ecosystem may define
registry replacement, authentication, withdrawn releases, feature unification,
platform predicates, source revisions, patches, peer contexts, binary
artifacts, build roles, and lockfile compatibility.

Reimplementing those rules in each Starlark module would create a second
package manager that can disagree with the native one. It would also make
native command-line workflows and Once builds observe different dependency
graphs.

The [PubGrub version-solving algorithm](https://github.com/dart-lang/pub/blob/master/doc/solver.md)
remains useful when an ecosystem has no authoritative solver, or for explaining
conflicts in normalized constraints. It is an optional backend, not the module
contract. Hex and Swift Package Manager already use PubGrub-family solving, so
Once should import their result instead of solving it again.

## Evidence From Other Build Systems

[Bazel](https://bazel.build/) integrations are strongest when the native
manager resolves and an extension creates repositories and typed targets.
[Crate Universe](https://bazelbuild.github.io/rules_rust/crate_universe_bzlmod.html)
ingests Cargo manifests and a lockfile, then generates Bazel targets with
separate normal, procedural macro, and build-script dependency roles.

[Buck2](https://buck2.build/) recommends using the native Go resolver and
[`gobuckify`](https://buck2.build/docs/users/languages/go/third_party_packages/)
to generate ordinary third-party targets. Its
[execution dependencies](https://buck2.build/docs/rule_authors/configurations/#execution-deps)
remain distinct from target-platform dependencies. The useful lesson is one
synthetic node per locked package with explicit dependency roles.

[Nx](https://nx.dev/) delegates installation and version selection to package
managers, then its
[lockfile importer](https://github.com/nrwl/nx/blob/master/packages/nx/src/plugins/js/lock-file/lock-file.ts)
and [package graph parser](https://github.com/nrwl/nx/blob/master/packages/nx/src/plugins/js/lock-file/npm-parser.ts)
create external graph nodes for tracking and pruning. Its name-and-version
identity is intentionally sufficient for invalidation but too lossy for builds.
Once preserves source, integrity, feature, platform, patch, and role context in
target attributes and labels.

## Lock And Source Model

The native lockfile remains the ecosystem source of truth. Checked-in graph
snapshots bind the exact manifest and lock text that produced them. Cargo
metadata snapshots also bind every resolver input plus feature and target
selection. These records supplement rather than replace the native lock.
A future resolution record may add adapter version, resolver toolchain
identity, exact source identities, patches, platform predicates, and the
expanded graph digest.

Ordinary builds are read-only. They import a locked graph and consume already
materialized exact sources. Source acquisition is a separate policy boundary:

- registry archives require the native integrity value;
- version-control sources require an exact revision;
- Zig remote packages require the content multihash from `build.zig.zon`;
- local path dependencies preserve their normalized workspace path;
- binary artifacts require the checksum recorded by the native ecosystem.

A later explicit dependency update command should own lockfile mutation,
source acquisition, dry runs, whole-graph updates, and package-specific
updates. It should invoke the native manager and then re-import the locked
graph. Builds and graph queries must never silently rewrite locks.

## Ecosystem Lowering

### Rust

Cargo runs metadata in locked mode. Each resolved crate becomes a Rust crate or
procedural macro target. Normal and build dependencies remain distinct.
Checksums, source identifiers, features, target conditions, build scripts, and
workspace direct-dependency aliases remain typed metadata. The dependency-set
owner aggregates compiled providers, while crate actions are scheduled through
the graph instead of a serial loop inside one target.

### Elixir

Mix and Hex own `mix.lock`. Each locked application becomes an Elixir package
target with its manager, version or revision, checksum, dependency edges, and
vendored source root. Mix, Rebar, Make, and custom compilation behavior must be
represented explicitly. Unsupported opaque behavior should fail instead of
running an undeclared shell command. A future package-fixup target can make
that behavior explicit.

### Swift

Swift Package Manager owns `Package.resolved` and package structure. The
adapter combines pins with locked package inspection and emits queryable
package identities with exact revisions, versions, registry checksums, and
transitive edges. The current build boundary invokes one locked native package
build for an explicit list of static products. Resource bundles, binary
artifacts, and build-tool plug-ins require new typed provider fields before
they can be lowered safely.

### Zig

`build.zig.zon` owns dependency aliases, local paths, source locations, lazy
flags, and content multihashes. The adapter parses the locked package graph,
maps complete materialized source trees to Zig module targets, and keeps
edge-local import names. A module path override handles packages whose public
module is not the conventional `src/root.zig`. Arbitrary `build.zig` execution
remains an explicit opaque fallback because it is executable build logic, not
dependency metadata.

## Cache And Scheduling Invariants

- Every resolved package is a distinct graph node.
- Package identity includes the native locked instance, not only display name
  and version.
- Dependency roles remain typed.
- Independent packages become schedulable concurrently.
- A package action should depend on its own minimal locked metadata, source,
  toolchain, configuration, and direct providers.
- A lockfile change should not invalidate unrelated package actions when their
  normalized records are unchanged.
- Graph expansion is deterministic and rejects duplicate labels.
- Native network access requires explicit opt-in when a locked Swift package is
  not fully vendored. Once does not independently sandbox the package manager.

## Diagnostics And Agent Use

Resolver target kinds are discoverable through schema queries and ship
runnable examples. Resolver-generated attributes are declared in schemas so
validation and graph editing stay typed. Synthetic targets appear in ordinary
graph queries, which lets humans and coding agents inspect the exact package
nodes and edges without reading generated build files.

## Alternatives Rejected

### A Starlark-first version solver

This duplicates native ecosystem semantics and produces lockfiles that native
tools may not accept.

### One package-manager action per dependency set

This hides the package graph, serializes independent work, and invalidates the
whole dependency set for small changes.

### Checked-in generated Once manifests

This makes manifest, native lock, generated targets, and source state drift
independently. Deterministic graph expansion provides reviewable query output
without another canonical file.

### Name-and-version tracking nodes

This is useful for coarse affected analysis but cannot distinguish source,
feature, platform, patch, peer, and build-role variants needed for correct
builds.
