# `cmake_target`

Resolver-generated logical CMake target.

## Description

`cmake_workspace` creates these targets from a checked configured graph. They
make native CMake names, dependency edges, sources, artifacts, include
directories, definitions, and configuration identity available to ordinary
Once graph queries.

The target is resolver-owned. Do not declare it directly in `once.toml`. It
does not build independently; use the paired
[`cmake_project`](/reference/prelude/cmake_project) target.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `cmake_name` | string | yes |  | Original configured CMake target name |
| `cmake_type` | string | yes |  | Configured CMake target type |
| `artifacts` | list&lt;string&gt; | no | `[]` | Normalized configured artifact paths |
| `include_dirs` | list&lt;string&gt; | no | `[]` | Normalized configured include directories |
| `compile_definitions` | list&lt;string&gt; | no | `[]` | Configured compile definitions |
| `snapshot_fingerprint` | string | yes |  | Canonical fingerprint shared by imported targets |
| `_cmake_resolved` | bool | resolver-owned | `false` | Prevents direct manifest authoring |

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `cmake_target` | Configured logical CMake dependencies |

## Providers

The target emits `cmake_target` with its logical metadata and dependency
identifiers.

## Capabilities

This target has no executable capability.

## Query Example

```sh
once query targets --kind cmake_target
once query target cmake-greeting
once query 'MATCH (root:Target {id: "cmake-greeting"})-[:DEPENDS_ON*]->(dep:Target) RETURN dep.id, dep.kind'
```

See the [CMake guide](/guide/graph/cmake) for the complete build and snapshot
workflow.
