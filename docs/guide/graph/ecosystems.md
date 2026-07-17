---
next: false
---

# Ecosystems

An ecosystem is a set of target kinds for one language, platform, or build
domain. It gives Once enough information to validate dependencies, explain
what a target can do, and cache work from declared inputs and outputs.

## Choose Between a Typed Target and a Script

Use an ecosystem target kind when Once should understand the artifact and its
relationships. This is a good fit when you want to:

- query targets and capabilities before doing work;
- validate dependencies and attributes early;
- reuse build outputs across targets or machines;
- expose focused build, run, and test operations to coding agents.

Use a [script target](/guide/scripted/) when an existing tool remains
the source of truth, the workflow is still exploratory, or the native feature
you need has not been modeled. Scripts still participate in the graph, so a
project can begin there and adopt typed targets one boundary at a time.

## Full Ecosystem Guides

Full build ecosystems have incremental guides with a first project,
query-before-build workflow, current limitations, and follow-up steps:

- [Apple](/guide/graph/apple) covers libraries, frameworks, applications, and
  tests written in Swift, Objective-C, C, and C++.
- [Android](/guide/graph/android) covers resources, Java and Kotlin libraries,
  application packages, and host or device tests.
- [C and C++](/guide/graph/c) covers headers, source compilation, static
  libraries, and native consumers.
- [Elixir](/guide/graph/elixir) covers compiled applications and ExUnit tests.
- [Rust](/guide/graph/rust) covers libraries, binaries, tests, procedural
  macros, Cargo dependencies, and native mobile outputs.
- [Zig](/guide/graph/zig) covers modules, binaries, tests, libraries, and C or
  C++ dependencies.

## Test Runner References

Test-only integrations have focused reference pages with prerequisites,
copyable declarations, runnable starters, and first-run commands:

- [pytest](/reference/prelude/pytest_test) covers Python test discovery,
  exact execution, and automatic file or case batching.
- [Ruby Specification](/reference/prelude/rspec_test) and
  [Minitest](/reference/prelude/minitest_test) cover Ruby test suites.
- [Vitest](/reference/prelude/vitest_test) and
  [Jest](/reference/prelude/jest_test) cover JavaScript and TypeScript test
  suites.

Continue with [Testing and Scheduling](/guide/graph/testing) for the shared
first-run workflow, affected selection, exact execution, and automatic
batching across these runners.

Use the [target kind index](/reference/prelude/) after choosing an ecosystem.
It lists the exact schema, dependencies, capabilities, outputs, and current
limitations for every kind.

## Shared Mobile Code

Some target kinds cross platform boundaries while preserving normal graph
dependencies:

- [`swift_android_library`](/reference/prelude/swift_android_library) packages
  Swift code for an Android application.
- [`kotlin_apple_framework`](/reference/prelude/kotlin_apple_framework)
  produces a Kotlin/Native framework for an Apple target.
- [`rust_mobile_library`](/reference/prelude/rust_mobile_library) produces the
  Apple or Android native library variant requested by its consumer.

Use these after the application builds without shared code. That sequence
keeps toolchain and linking problems separate from the first graph setup.

## Adopt an Ecosystem Incrementally

1. Pick the smallest artifact that has stable inputs and outputs.
2. Declare one target and run `once query schema <kind>`.
3. Run `once query capabilities <target>` and build that same target.
4. Connect one consumer through `deps` and query the resulting graph.
5. Add run or test targets only after the artifact build is reliable.
6. Keep unsupported edges behind scripts until a typed target kind covers
   them.

This sequence leaves the native package manager or build system in place for
the parts Once does not own. A project does not need to move an entire
ecosystem at once.

A coding harness can also fetch the authoritative external rule or plugin,
query Once's live module-authoring contract, and maintain that local target
kind for the project. This is useful when the project needs a narrow dependency
slice and a general built-in integration would be unnecessary. See
[Coding harnesses](/guide/harness#adopt-an-unfamiliar-external-rule).

## Check the Boundary Before Adopting

Feature coverage varies by ecosystem. Before moving a production workflow,
check the target kind reference for:

- supported source, resource, and dependency shapes;
- ownership of third-party dependency resolution;
- required compilers and platform tools;
- cacheable artifacts and non-cacheable runtime effects;
- unsupported attributes and whether they fail validation;
- editor or language-service integration the existing workflow still needs.

Unsupported does not have to mean blocked. Keep that behavior in a script,
define a checked-in local module, or contribute the missing typed behavior.
The important part is to keep the boundary visible so readers and agents can
tell which system owns each step.
