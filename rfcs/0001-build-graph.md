# RFC 0001: Once Build Graph

## Summary

Once is expanding around a graph and action model for repository automation.
Scripts remain the adapter for existing work: teams can make automation
cacheable first, then move the same work into typed graph rules when they need
richer target relationships, queries, diagnostics, and agent-driven edits.

The first product slice should focus on Apple platforms. The goal is to model
Apple libraries, frameworks, applications, application runs, and tests well
enough that Once can build and run real iOS, macOS, watchOS, tvOS, and
visionOS workflows over time. Buck2 is the strongest architectural inspiration,
especially its Rust core, Starlark rule layer, phase model, query model, and
incremental computation engine. Bazel and Buck Apple rules are the strongest
domain references for target shapes, providers, toolchain concerns, and common
Apple build products.

Once should be Buck2-inspired, not Buck2-compatible. It should borrow the good
architecture while designing the graph, edit API, diagnostics, and query model
for coding agents from the beginning.

## Goals

- Model Apple targets as typed graph data, not as arbitrary executable graph
  construction.
- Build Apple libraries, frameworks, bundles, applications, and test bundles.
- Run application targets through an explicit run capability that depends on
  build outputs.
- Run tests through an explicit test capability with structured test results.
- Keep scripts as first-class adapters into the action model instead of
  replacing the existing `once exec` workflow.
- Give agents schema introspection, structured graph operations, structured
  diagnostics, repair suggestions, and explanation queries.
- Preserve Once's action cache, content-addressed storage, remote execution,
  and runtime API as the execution substrate.

## Non-Goals

- Buck2, Bazel, BUILD file, or Starlark compatibility.
- A full external dependency implementation in the first code slice.
- Arbitrary graph macros in target declarations.
- Direct agent editing of TOML text as the primary workflow.
- Shipping every Apple rule shape before the graph, capability, and query model
  exist.

## Product Model

Once has a small set of durable concepts:

1. Targets: named units in the workspace.
2. Capabilities: operations a target exposes, such as `build`, `run`, and
   `test`.
3. Actions: concrete executable work with inputs, outputs, environment,
   platform requirements, and cache identity.
4. Rules: typed logic that validates targets and lowers capabilities into
   actions.
5. Scripts: the least typed rule-backed adapter for existing executable files.

The Apple platform is the first full build graph proving ground because it
requires the system to model real build-system complexity: compile actions,
linking, bundles, resources, code signing, entitlements, provisioning, platform
selection, simulators, app installation, app launch, test execution, and
structured test results.

## Graph Source Of Truth

Canonical `once.toml` files are the at-rest source of truth. The graph is
loaded from files, but agents should not edit those files directly. Agents use
validated graph operations that rewrite canonical files and return semantic and
textual diffs for review.

Target ids stay path-like for human and CLI ergonomics:

```text
apps/ios/app
apps/ios/AppKit
apps/ios/AppTests
packages/auth/AuthFramework
```

Internally, every target has a structured package and name:

```text
package = "apps/ios"
name = "app"
id = "apps/ios/app"
```

Target declarations use one canonical shape:

```toml
[[target]]
name = "App"
kind = "apple_application"
deps = ["apps/ios/AppKit"]

[target.attrs]
platform = "ios"
bundle_id = "dev.once.App"
minimum_os = "17.0"
resources = ["Resources/**"]
entitlements = "App.entitlements"
```

This example is illustrative. The implementation should define a typed graph
value model rather than treating attrs as arbitrary string maps.

## Target Capabilities

Rules expose capabilities. Capabilities are the contract between CLI commands,
agents, analysis, and execution.

- `build`: produce artifacts such as static libraries, dynamic libraries,
  frameworks, `.app` bundles, `.xctest` bundles, generated sources, debug
  symbols, and packaged archives.
- `run`: execute or launch a built product. For Apple apps this can install and
  launch on a simulator, device, or local macOS environment.
- `test`: execute tests and emit structured test results.

A target can expose more than one capability. For example:

- `apple_framework`: `build`
- `apple_application`: `build`, `run`
- `apple_test_bundle`: `build`, `test`
- `script`: `run`, and optionally `test` if the script reports tests

Ambiguous capability selection must fail with a structured error that lists the
available capabilities. Once should not silently choose a default when a target
supports multiple workflows.

`run` must declare the build outputs it needs. An app run should depend on the
`.app` bundle or equivalent artifact through the action graph, not through a
private rule side effect. That makes questions like "why did running this app
rebuild the framework?" answerable.

Every capability exposes named output groups. Apple examples include:

- `default`
- `binary`
- `bundle`
- `framework`
- `dsyms`
- `swiftmodule`
- `generated_sources`
- `coverage`
- `test_results`

## Apple Rule Families

The initial Apple rule families should be small but real:

- `apple_library`: compiles Swift, Objective-C, C, and C++ sources into a
  linkable module or library.
- `apple_framework`: builds a framework product, including module metadata,
  resources when applicable, and debug symbol outputs.
- `apple_application`: builds an application bundle with resources,
  entitlements, Info.plist processing, code signing inputs, and platform
  metadata.
- `apple_test_bundle`: builds and runs XCTest-style test bundles, including
  app-hosted tests when needed.
- `script`: keeps existing executable automation available as graph targets
  during migration.

The RFC does not require copying Bazel or Buck rule names or attributes. It
requires studying their Apple rules for the provider boundaries and build
products that matter:

- source compilation, module interfaces, headers, and generated sources
- platform and architecture selection
- resources and asset catalogs
- Info.plist and bundle metadata
- entitlements, provisioning, signing identity, and signing mode
- framework and application bundle assembly
- XCTest bundles, test hosts, and test plans
- simulator or device selection for run and test

## Rule Contract

Starlark is the likely rule language, following Buck2's split between a Rust
core and Starlark rules. The rule contract matters more than the interpreter
choice.

Every rule must expose a machine-readable schema:

- attributes, types, defaults, required fields, and docs
- configurable values where attributes can vary by platform or constraint
- dependency attributes and expected provider types
- produced providers
- supported capabilities
- output groups per capability
- validation diagnostics and candidate repair operations
- examples that agents can retrieve before authoring targets

Rule schemas are mandatory for first-party and third-party rules. Agents should
introspect schemas before creating or editing targets.

## Configuration And Toolchains

The graph has explicit phases inspired by Buck2:

1. Unconfigured graph: declared targets and dependencies.
2. Configured graph: configurable attributes, platforms, constraints, and
   toolchains resolved.
3. Action graph: concrete actions, inputs, outputs, resources, and execution
   strategy.
4. Execution outcomes: cache hits, misses, logs, outputs, diagnostics, test
   results, and provenance.

Configurable values, similar to `select`, are first-class graph data. Full
platform transitions can come later, but Apple platform selection must be a
first-class design concern from the start.

Toolchains are graph entities selected during configuration. Apple toolchains
must describe Xcode, SDKs, platform triples, Swift and Clang settings, signing
tools, simulator availability, and provider constraints. The toolchain model
should work locally and remotely, even when some actions must run on macOS.

## Presets, Subtargets, And Visibility

Once should not support arbitrary target-declaration macros. Reusable
conventions should be modeled with typed presets and rule-generated subtargets.

Presets provide schema-checked defaults for matching rule types. For example, an
organization can define common Apple bundle settings, default Swift compiler
flags, visibility, signing policy, test environment, or dependency groups.

Subtargets are named outputs or views generated by a rule. For Apple platforms,
subtargets can expose products such as `App#bundle`, `App#dsyms`,
`Framework#swiftmodule`, or `Tests#coverage`. Subtargets can expose their own
capabilities when useful.

Visibility and package boundaries are part of the graph from the beginning.
Automated dependency repair must not add dependencies that violate ownership or
visibility.

## Agent API

Agents interact with the graph through operations, not TOML edits. The same
operation model should be available through JSON-RPC and CLI wrappers.

Representative operations:

- `schema.rule(kind: "apple_application")`
- `graph.add_target`
- `graph.add_dep`
- `graph.set_attr`
- `graph.apply_preset`
- `graph.rename_target`
- `graph.move_target`
- `graph.transaction`
- `query.capabilities`
- `query.why_changed`
- `query.why_dep`
- `query.affected_tests`
- `query.cost`

Graph edits are transactional. A transaction validates schema, labels,
visibility, configuration constraints where possible, and rule constraints
before writing files. A successful transaction returns:

- semantic graph diff
- textual file diff
- changed target ids
- affected configured graph keys
- likely affected build, run, and test capabilities

Agents should be able to preview an edit without committing it.

## Diagnostics And Repairs

Once diagnostics are structured first and raw stderr second. Every diagnostic
should identify as much of the following as possible:

- target id
- capability
- action
- rule kind
- source file or manifest location
- tool message
- provider or toolchain context
- candidate repairs

Apple-specific repairs should include:

- missing dependency from observed file access
- missing resource declaration
- missing framework or library dependency
- incompatible platform or SDK setting
- signing and provisioning mismatch
- test host mismatch
- simulator destination mismatch
- generated source missing from declared outputs

Repair loops must cover missing dependencies, build errors, and cache misses.
The missing-dependency loop is the flagship case: sandboxed or remote execution
observes true file accesses, maps files back to owning targets, checks
visibility, and proposes a structured `graph.add_dep` operation.

## Queries

The user-facing query surface should be `once query`, with phase-aware query
modes rather than Buck or Bazel command names.

Required query families:

- `once query targets`: list declared or configured targets.
- `once query capabilities <target>`: list build, run, test, output groups,
  and required inputs.
- `once query why-changed <target>`: explain rebuilds, cache reuse, and cache
  misses.
- `once query why-dep <from> <to>`: explain dependency provenance through
  direct deps, presets, rule-generated deps, toolchains, or observed repairs.
- `once query affected-tests`: combine graph reachability, observed inputs,
  historical results, and rule metadata.
- `once query cost`: estimate rebuild and test cost from historical action
  timings, cache hit rates, and local or remote outcomes.

## Execution And Snapshots

Once actions remain content-addressed and cacheable. Local execution, remote
execution, and hybrid execution are first-class action properties. Apple support
must account for platform constraints, especially macOS-only work and simulator
or device interactions.

Configured graph snapshots and action graph snapshots should be
content-addressed. This enables:

- reproducible query replay
- speculative copy-on-write graph edits
- comparing build variants
- discarding losing agent attempts without mutating canonical files
- explaining what changed between snapshots

## External Dependencies

External dependencies should use a registry-agnostic module model with provider
adapters. Apple support needs SwiftPM, git, binary frameworks, local packages,
and language ecosystems used by mixed Apple projects.

This RFC does not specify the full resolver. It requires the build graph design
to leave room for a module graph with lockable versions, provenance,
checksums, visibility, and rule-provided providers.

## First Implementation Slice

The first implementation after this RFC should not start with Starlark rule
execution. It should start with the graph model and queries needed to make the
Apple target model inspectable.

Suggested order:

1. Define typed target, label, attr, capability, output group, provider, and
   diagnostic models in Rust.
2. Load canonical `once.toml` target declarations without changing existing
   script execution.
3. Represent script adapter targets so scripts appear in graph queries.
4. Add schema introspection for a small built-in Apple rule set.
5. Add `once query targets` and `once query capabilities`.
6. Add configured graph and action graph placeholders for Apple build, run, and
   test capabilities.
7. Add structured diagnostics before adding broad rule execution.

The first proof should show:

- an Apple library target can expose a build capability
- an Apple framework target can expose framework and debug-symbol outputs
- an Apple app target can expose build and run capabilities
- an Apple test target can expose build and test capabilities
- a script target still works as a migration target

## Open Questions

- Which Apple rule names should be final once implementation begins?
- How much of Xcode project generation should Once avoid, replace, or
  interoperate with?
- Should SwiftPM packages be imported as external modules, generated graph
  targets, or both?
- Which signing workflows should be supported first: ad hoc, development,
  CI-managed, or provider-managed?
- How should device and simulator selection be represented in run and test
  capabilities?
- How should remote macOS execution providers declare available Xcode and
  simulator capabilities?
