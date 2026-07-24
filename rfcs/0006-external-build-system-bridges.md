# Request for Comments 0006: External Build-System Bridges

## Summary

Once should provide an incremental bridge for repositories whose existing
build system remains authoritative. The first bridge integrates CMake in two
complementary ways:

1. `cmake_project` configures and builds declared CMake products as one
   cacheable Once action.
2. `cmake_workspace` imports a checked CMake graph snapshot as typed,
   queryable `cmake_target` nodes.

This keeps the migration path close to annotated scripts. Teams can first move
an existing build command behind a typed action, then expose the logical graph
without asking Once to reinterpret `CMakeLists.txt` or replace CMake.

## Decision

CMake behavior belongs in built-in Starlark target kinds. The Rust
implementation supplies only generic graph resolution, schema, action, and
filesystem primitives.

The execution bridge and graph import deliberately have different
granularity:

- Execution is coarse. Configure and build run in one action because the
  generated build tree contains state that cannot yet be modeled as portable
  independent Once actions.
- Discovery is fine-grained. A normalized snapshot creates one
  `cmake_target` node per selected configured CMake target so queries can
  inspect names, dependency edges, sources, artifacts, includes, definitions,
  and configuration provenance.

Logical imported targets do not declare build actions. The `cmake_project`
owner remains the executable cache boundary.

## Why CMake Remains Authoritative

CMake is a programmable configuration language with generators, toolchain
files, platform checks, cache variables, custom commands, policies, and
project-defined functions. Parsing `CMakeLists.txt` would not reproduce the
configured graph.

The bridge therefore consumes CMake's
[file-based application programming interface](https://cmake.org/cmake/help/latest/manual/cmake-file-api.7.html).
An explicit refresh script configures the project, asks CMake for its
codemodel, cache, configuration inputs, and toolchain records, then writes a
normalized snapshot.

Ordinary graph loading never configures CMake and never edits the snapshot.
This keeps queries deterministic and makes configuration changes reviewable.

## Execution Contract

`cmake_project` declares:

- the package-relative source directory;
- all source and configuration inputs;
- the generator, build configuration, and arguments;
- optional native build targets;
- exact products expected under the generated build directory;
- public headers, include directories, definitions, link options, and data.

The target resolves and probes CMake and the generated-build program during
analysis. Their versions, the generator, and the build configuration form part
of the toolchain identity.

The action writes a small CMake driver and invokes CMake directly with an
argument list. The driver performs configure followed by `cmake --build`.
It does not use a shell. The generated build directory is cleaned before a
cache miss executes, and only declared product files become action outputs.
Each product is then staged into the stable target output directory.

Platform tokens let one manifest describe common product names:

- `{static_prefix}` and `{static_suffix}`;
- `{shared_prefix}` and `{shared_suffix}`;
- `{exe_suffix}`.

The target exposes the declared products and native link information through
the `cmake_project`, `c_provider`, `native_linkable`, and `apple_linkable`
provider contracts.

## Snapshot Contract

The normalized snapshot uses schema `once.cmake.snapshot.v1`. It records:

- exact text for every workspace configuration input reported by CMake;
- source directory, generator, and build configuration selection;
- a canonical snapshot fingerprint;
- normalized CMake version, cache, and toolchain metadata;
- configured target names, types, dependency edges, sources, artifacts,
  includes, and definitions;
- configured CTest records for inspection.

`cmake_workspace` compares the snapshot provenance with its declared resolver
inputs. Graph loading fails when an input is missing, has changed, or the
configuration selection differs. This prevents an old logical graph from
silently describing a new build.

The `exports` attribute selects configured CMake targets by native or generated
name. The resolver includes their transitive dependencies and connects the
workspace target to the selected roots.

## Update Workflow

Snapshot refresh is an explicit repository operation:

1. Run the annotated refresh script with the repository toolchain.
2. Review the changed normalized snapshot for the selected refresh host and
   toolchain.
3. Query the imported target graph.
4. Build the coarse `cmake_project` target.

Builds and graph queries never refresh configuration implicitly. A continuous
integration check can run the refresh script and fail when the checked
snapshot changes.

## Agent Use

All three target kinds are available through target-kind and schema queries.
They share a runnable starter containing a CMake project, a refresh script,
and a checked snapshot. Coding agents can discover the contract, materialize
the starter, inspect logical CMake targets, and reproduce the same operations
from the command line.

The snapshot gives agents a bounded structural view without requiring them to
parse CMake source or inspect a mutable generated build tree.

## Current Boundaries

- A CMake build is one action, not one Once action per compile or link step.
- Imported `cmake_target` nodes are query metadata and do not build
  independently.
- Product paths are declared explicitly because generated layouts vary by
  project and generator.
- CTest records are captured in the snapshot but do not yet expose the Once
  test capability.
- The shipped starter uses Ninja. Other generators require compatible tools
  and accurate product paths.
- Configuration input provenance covers workspace text files reported by
  CMake. Projects must also declare ordinary source, header, data, and
  generated-input patterns on `cmake_project`. Configuration files outside the
  workspace require an explicit repository policy.

## Alternatives Rejected

### Parse CMake source in Once

This cannot reproduce configured behavior and would make Once a second,
incomplete CMake interpreter.

### Convert every generated command into an Once action immediately

Generated commands depend on configuration state, dependency discovery,
response files, depfiles, and generator-specific rules. Importing them without
a complete model would create incorrect cache boundaries.

### Keep only an annotated build script

Scripts remain a valid migration ramp, but they do not provide a typed product
contract or a queryable logical target graph.

### Configure CMake during every graph query

This would make discovery host-dependent, potentially expensive, and capable
of changing generated state. A checked snapshot keeps graph loading
deterministic.
