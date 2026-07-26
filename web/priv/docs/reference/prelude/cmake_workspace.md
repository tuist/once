# `cmake_workspace`

Imports a checked CMake graph snapshot.

## Description

The resolver reads a normalized snapshot produced from CMake's
[file-based application programming interface](https://cmake.org/cmake/help/latest/manual/cmake-file-api.7.html).
It validates exact configuration input text and selection provenance, then
creates one queryable [`cmake_target`](/reference/prelude/cmake_target) per
selected configured target and transitive dependency.

This target does not configure or build the project. Pair it with
[`cmake_project`](/reference/prelude/cmake_project) for execution.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `snapshot` | string | yes |  | Package-relative normalized CMake snapshot |
| `resolver_inputs` | list&lt;string&gt; | no | `[]` | Snapshot plus every text configuration input supplied to the resolver |
| `source_dir` | string | no | `.` | Source directory recorded in selection provenance |
| `generator` | string | no | `Ninja` | CMake generator recorded in selection provenance |
| `build_type` | string | no | `Debug` | CMake build configuration recorded in selection provenance |
| `exports` | list&lt;string&gt; | no | snapshot exports | Native CMake names or generated Once names to expose |
| `_cmake_resolved` | bool | resolver-owned | `false` | Marks a workspace expanded by the resolver |
| `_cmake_snapshot_fingerprint` | string | resolver-owned | empty | Canonical fingerprint from the checked snapshot |
| `_cmake_exports` | list&lt;string&gt; | resolver-owned | `[]` | Generated names selected as roots |

The underscore-prefixed attributes are produced by the resolver and should not
be set in `once.toml`.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `cmake_target` | Resolver-generated logical CMake targets |

## Providers

The target emits `cmake_workspace` with the snapshot fingerprint, selected
exports, and imported target provider records.

## Capabilities

This target has no executable capability.

## Example

```toml
[[target]]
name = "CMakeGraph"
kind = "cmake_workspace"
srcs = ["CMakeLists.txt", "cmake-snapshot.json"]

[target.attrs]
snapshot = "cmake-snapshot.json"
resolver_inputs = ["CMakeLists.txt", "cmake-snapshot.json"]
generator = "Ninja"
build_type = "Release"
exports = ["greeting"]
```

Graph loading rejects the snapshot when a bound input or selection value
changes. See the [CMake guide](/guide/graph/cmake) for the explicit refresh
workflow.
