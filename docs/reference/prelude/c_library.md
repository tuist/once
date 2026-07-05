# `c_library`

C and C++ static library provider.

## Description

Compiles C and C++ sources into object files, archives them into a static
library when sources are present, and exposes headers plus link inputs through
the `c_provider` record. Header-only targets are valid and still propagate
include directories, definitions, and data.

Zig target kinds consume this provider directly. Other native target kinds can
also read the same record fields by convention.

Object outputs preserve the workspace-relative source path under `objects/`, so
sources with similar names such as `foo-bar.c` and `foo_bar.c` do not collide.
The C compiler is probed for every source-bearing target. The C++ compiler is
only probed when at least one source uses a C++ extension. Set
`archiver_identity` when the selected archiver path is mutable and the action
cache should distinguish different archiver builds.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `compiler` | string | no | `cc` on PATH | C compiler path |
| `cxx_compiler` | string | no | `c++` on PATH | C++ compiler path |
| `archiver` | string | no | `ar` on PATH | Static library archiver path |
| `archiver_identity` | string | no | empty | Stable archiver identity, such as a version or content digest, folded into action cache keys |
| `hdrs` | list&lt;string&gt; | no | `[]` | Public header files exposed to dependents |
| `header_globs` | list&lt;string&gt; | no | `[]` | Public header glob patterns exposed to dependents |
| `includes` | list&lt;string&gt; | no | `[]` | Public include directories propagated to dependents |
| `quote_includes` | list&lt;string&gt; | no | `[]` | Public quote include directories propagated as regular include directories |
| `system_includes` | list&lt;string&gt; | no | `[]` | Public system include directories propagated to dependents |
| `framework_includes` | list&lt;string&gt; | no | `[]` | Public framework search directories propagated to dependents |
| `defines` | list&lt;string&gt; | no | `[]` | Definitions propagated to dependent compile actions |
| `copts` | list&lt;string&gt; | no | `[]` | Compiler flags used by this target |
| `linkopts` | list&lt;string&gt; | no | `[]` | Linker flags propagated to dependents |
| `dynamic_libraries` | list&lt;string&gt; | no | `[]` | Prebuilt dynamic libraries propagated to dependents |
| `android_abi` | string | no | empty | Android [Application Binary Interface](https://developer.android.com/ndk/guides/abis) directory for dynamic libraries |
| `data` | list&lt;string&gt; | no | `[]` | Runtime data globs propagated to dependents |
| `env` | map&lt;string, string&gt; | no | `{}` | Environment variables passed to compile and archive actions |
| `output_name` | string | no | target name | Static library output name without prefix or extension |

## Dep Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `c_provider` | C provider dependencies whose headers and link inputs are propagated |

## Providers

The target emits `c_provider`, `native_linkable`, `apple_linkable`, and
`android_native_library`.

## Provider Record

| Field | Type | Meaning |
| --- | --- | --- |
| `archive` | string | Static library output when sources are present |
| `static_library` | string | Alias for `archive` |
| `static_libraries` | list&lt;string&gt; | Direct static libraries exposed by this target |
| `dynamic_libraries` | list&lt;string&gt; | Direct dynamic libraries exposed by this target |
| `objects` | list&lt;string&gt; | Object files compiled from this target's sources |
| `headers` | list&lt;string&gt; | Direct public headers |
| `include_dirs` | list&lt;string&gt; | Direct public include directories |
| `quote_include_dirs` | list&lt;string&gt; | Direct quote include directories passed as regular include dirs |
| `system_include_dirs` | list&lt;string&gt; | Direct system include directories |
| `framework_include_dirs` | list&lt;string&gt; | Direct framework search directories |
| `defines` | list&lt;string&gt; | Direct definitions |
| `copts` | list&lt;string&gt; | Direct compiler options |
| `linkopts` | list&lt;string&gt; | Direct linker options |
| `transitive_headers` | list&lt;string&gt; | Direct and dependency headers |
| `transitive_include_dirs` | list&lt;string&gt; | Direct and dependency include directories |
| `transitive_quote_include_dirs` | list&lt;string&gt; | Direct and dependency quote include directories |
| `transitive_system_include_dirs` | list&lt;string&gt; | Direct and dependency system include directories |
| `transitive_framework_include_dirs` | list&lt;string&gt; | Direct and dependency framework search directories |
| `transitive_defines` | list&lt;string&gt; | Direct and dependency definitions |
| `transitive_static_libraries` | list&lt;string&gt; | Static libraries for dependents |
| `transitive_dynamic_libraries` | list&lt;string&gt; | Dynamic libraries for dependents |
| `transitive_linkopts` | list&lt;string&gt; | Linker flags for dependents |
| `transitive_data` | list&lt;string&gt; | Runtime data propagated to dependents |
| `transitive_archives` | list&lt;string&gt; | Alias for transitive static library archive paths |
| `android_native_libraries` | list&lt;record&gt; | Direct Android native-library records when `android_abi` is set |
| `transitive_android_native_libraries` | list&lt;record&gt; | Direct and dependency Android native-library records |
| `affected_inputs` | list&lt;string&gt; | Source, header, and runtime inputs associated with this provider |
| `default_output` | string | Static library output when sources are present |

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `library` |

## Example

```toml
[[target]]
name = "native_math"
kind = "c_library"
srcs = ["src/*.c"]

[target.attrs]
hdrs = ["include/native_math.h"]
includes = ["include"]
```
