# `cmake_project`

Cacheable coarse build boundary for an existing CMake project.

## Description

The target keeps CMake authoritative. It resolves and probes CMake plus the
generated-build program, writes a portable CMake driver, and runs configure
followed by `cmake --build` in one action. It stages only explicitly declared
product files into the stable target output directory.

The target also exposes declared headers, include directories, definitions,
link options, libraries, and data through the shared native provider shape.
Use [`cmake_workspace`](/reference/prelude/cmake_workspace) when the configured
CMake graph should also be queryable.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `cmake` | string | no | `cmake` | CMake executable name or path |
| `source_dir` | string | no | `.` | Package-relative CMake source directory |
| `generator` | string | no | `Ninja` | CMake generated-build backend |
| `build_program` | string | no | `ninja` for Ninja | Generated-build executable name or path |
| `build_type` | string | no | `Debug` | CMake build configuration |
| `configure_args` | list&lt;string&gt; | no | `[]` | Additional CMake configure arguments |
| `build_targets` | list&lt;string&gt; | no | `[]` | Native CMake targets passed to the generated build |
| `build_args` | list&lt;string&gt; | no | `[]` | Additional arguments passed to `cmake --build` |
| `products` | list&lt;string&gt; | yes |  | Exact files expected under the generated build directory |
| `hdrs` | list&lt;string&gt; | no | `[]` | Public package-relative headers |
| `header_globs` | list&lt;string&gt; | no | `[]` | Public header patterns |
| `includes` | list&lt;string&gt; | no | `[]` | Public include directories |
| `defines` | list&lt;string&gt; | no | `[]` | Public preprocessor definitions |
| `linkopts` | list&lt;string&gt; | no | `[]` | Linker options propagated to dependents |
| `data` | list&lt;string&gt; | no | `[]` | Runtime data patterns propagated to dependents |
| `env` | map&lt;string, string&gt; | no | `{}` | Environment variables passed to configure and build |

Product paths support `{static_prefix}`, `{static_suffix}`,
`{shared_prefix}`, `{shared_suffix}`, and `{exe_suffix}`. Absolute paths and
paths that escape the generated build directory are rejected.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `c_provider` | Native headers, libraries, and data included in the CMake action input tree |

## Providers

The target emits `cmake_project`, `c_provider`, `native_linkable`, and
`apple_linkable`.

The provider includes `products`, direct and transitive headers, include
directories, definitions, static libraries, dynamic libraries, link options,
data, affected inputs, and `default_output`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `default`, `products`, `library` |

## Example

```toml
[[target]]
name = "CMakeProject"
kind = "cmake_project"
srcs = ["CMakeLists.txt", "include/**/*.h", "src/**/*.c"]

[target.attrs]
generator = "Ninja"
build_type = "Release"
build_targets = ["greeting"]
products = ["{static_prefix}greeting{static_suffix}"]
hdrs = ["include/greeting.h"]
includes = ["include"]
```

See the [CMake guide](/guide/graph/cmake) for the snapshot refresh and query
workflow.
