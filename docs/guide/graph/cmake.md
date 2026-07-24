---
prev: false
next: false
---

# CMake

Once can sit in front of an existing CMake project without replacing CMake.
The bridge gives the repository a typed, cacheable build boundary and a
queryable view of the configured CMake graph.

The two parts are intentionally separate:

- `cmake_project` runs configure and build as one Once action, then stages only
  declared products.
- `cmake_workspace` imports a checked snapshot as logical `cmake_target`
  nodes for graph queries.

This is the same migration direction as annotated scripts, with an explicit
product contract and structural graph data added around the existing build.

## Prerequisites

The starter uses CMake, Ninja, Python 3, and a C compiler:

```sh
cmake --version
ninja --version
python3 --version
cc --version
```

The repository toolchain pins CMake and Ninja. Projects can set explicit
`cmake` and `build_program` paths when they use different installations.

## Try the Starter

Materialize the complete example:

```sh
once edit materialize-example cmake_project cmake-project-minimal
```

The starter contains a static C library, a `cmake_project` build target, a
`cmake_workspace` graph target, an annotated snapshot refresh script, and a
checked snapshot.

Inspect both layers before building:

```sh
once query targets
once query schema cmake_project
once query schema cmake_workspace
once query schema cmake_target
```

The target list includes `CMakeProject`, `CMakeGraph`, and the imported
`cmake-greeting` logical target.

Build the declared archive:

```sh
once build CMakeProject
```

The stable product is written under
`.once/out/CMakeProject/products/`. Running the command again reuses the
cached coarse build action when its inputs and configuration have not changed.

## Declare the Build Boundary

A minimal target names every input and product visible to Once:

```toml
[[target]]
name = "CMakeProject"
kind = "cmake_project"
srcs = [
  "CMakeLists.txt",
  "cmake/**/*.cmake",
  "include/**/*.h",
  "src/**/*.c",
]

[target.attrs]
source_dir = "."
generator = "Ninja"
build_type = "Release"
build_targets = ["greeting"]
products = ["{static_prefix}greeting{static_suffix}"]
hdrs = ["include/greeting.h"]
includes = ["include"]
```

Configure and build execute together because the generated CMake build tree is
stateful. The action does not cache that whole tree. Only files listed in
`products` are outputs and are staged into the stable Once output directory.

Product paths are relative to the generated build directory. The portable
tokens are:

| Token | macOS and Linux | Windows |
| --- | --- | --- |
| `{static_prefix}` | `lib` | empty |
| `{static_suffix}` | `.a` | `.lib` |
| `{shared_prefix}` | `lib` | empty |
| `{shared_suffix}` | `.dylib` on macOS, `.so` on Linux | `.dll` |
| `{exe_suffix}` | empty | `.exe` |

Declare multiple products when one native target produces more than one file.
The first product is the default output. Static and dynamic libraries also
populate the shared native link provider so compatible Once targets can
consume them.

## Import the Logical Graph

CMake's
[file-based application programming interface](https://cmake.org/cmake/help/latest/manual/cmake-file-api.7.html)
is the configured source of truth. The starter refresh script asks CMake for
the codemodel, cache, input files, and toolchains, then normalizes them into a
checked [JavaScript Object Notation](https://www.json.org/json-en.html)
snapshot.

Declare the imported workspace beside the build target:

```toml
[[target]]
name = "CMakeGraph"
kind = "cmake_workspace"
srcs = [
  "CMakeLists.txt",
  "cmake/**/*.cmake",
  "cmake-snapshot.json",
]

[target.attrs]
snapshot = "cmake-snapshot.json"
resolver_inputs = [
  "CMakeLists.txt",
  "cmake/**/*.cmake",
  "cmake-snapshot.json",
]
source_dir = "."
generator = "Ninja"
build_type = "Release"
exports = ["greeting"]
```

`exports` accepts native CMake names or generated Once names. The resolver
imports each export plus its transitive dependencies. Every imported target
records its native name, configured type, dependencies, sources, artifacts,
include directories, definitions, and snapshot fingerprint.

Query the imported structure:

```sh
once query targets --kind cmake_target
once query target cmake-greeting
once query 'MATCH (root:Target {id: "cmake-greeting"})-[:DEPENDS_ON*]->(dep:Target) RETURN dep.id, dep.kind'
```

The logical targets are structural metadata. Build `CMakeProject`, not an
individual `cmake_target`.

## Refresh the Snapshot

Run the annotated script whenever CMake configuration inputs or selection
change:

```sh
once exec -- python3 scripts/refresh-cmake-snapshot.py
```

Then review and query the result:

```sh
git diff -- cmake-snapshot.json
once query targets --kind cmake_target
once build CMakeProject
```

The snapshot binds the exact text of every workspace configuration input
reported by CMake, plus `source_dir`, `generator`, and `build_type`. Graph
loading rejects a stale snapshot before analysis. This makes refresh explicit
and prevents an old logical graph from silently describing a changed project.

For continuous integration, run the refresh script on the selected refresh
host and toolchain, then fail when it changes the checked snapshot.

## Connect Native Consumers

`cmake_project` exposes public headers, include directories, definitions,
linker options, static libraries, dynamic libraries, and data through the same
native provider shape used by Once's C and C++ targets.

Declare these fields explicitly:

```toml
[target.attrs]
products = ["{static_prefix}engine{static_suffix}"]
hdrs = ["include/engine.h"]
includes = ["include"]
defines = ["ENGINE_PUBLIC=1"]
linkopts = ["-pthread"]
```

This metadata is not inferred from the snapshot yet. The build contract stays
reviewable and does not depend on host-specific configured paths.

## Current Limitations

- Configure and build form one cache boundary. Once does not schedule each
  generated compile and link command independently.
- Imported targets are query-only and cannot be built directly.
- Exact product paths must be declared. Multi-configuration generators may
  place products under configuration-specific directories.
- The snapshot records configured CTest tests for inspection, but it does not
  expose `once test` yet.
- Source and header patterns still belong on `cmake_project`; snapshot
  provenance covers workspace CMake configuration inputs, not every
  compilation input or configuration file outside the workspace.
- The generated build directory is cleaned before an uncached action, so CMake
  incremental state is not reused across cache misses.

Use an annotated script when the build does not yet have stable products.
Adopt `cmake_project` when those products are known, then add
`cmake_workspace` when agents and humans benefit from target-level queries.

## Target References

- [`cmake_project`](/reference/prelude/cmake_project) documents the executable
  build boundary and native provider.
- [`cmake_workspace`](/reference/prelude/cmake_workspace) documents snapshot
  validation and graph expansion.
- [`cmake_target`](/reference/prelude/cmake_target) documents imported logical
  target metadata.
