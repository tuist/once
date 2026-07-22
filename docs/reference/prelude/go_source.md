# `go_source`

Reusable Go package sources, embedded files, data, and dependency metadata.

`go_source` performs no compilation. Connect it to `go_library`, `go_binary`,
or `go_test` through the `embed` dependency role to compile its sources into
the consuming package.

## Common Go Attributes

The following attributes are also accepted by `go_library`, `go_binary`, and
`go_test`.

| Attribute | Type | Default | Description |
| --- | --- | --- | --- |
| `module_root` | string | manifest package | Workspace-relative Go module root |
| `package`, `package_root` | string | target package | Go package pattern, with `package_root` as the Buck2-compatible alias |
| `go_work` | string | empty | Workspace-relative `go.work` file, otherwise `GOWORK=off` |
| `mod_mode` | string | `vendor` | `vendor` or `readonly`; external modules require the vendor mode |
| `go` | string | Go on `PATH` | Go command override |
| `goos`, `goarch` | string | host | Destination operating system and architecture |
| `cc`, `cxx` | string | host commands | C and C++ compilers used by cgo |
| `importpath`, `importmap` | string | empty | Public and compiler import identities |
| `package_name` | string | empty | Buck2-compatible public import identity |
| `importpath_aliases` | list&lt;string&gt; | `[]` | Bazel-compatible migration aliases |
| `embedsrcs`, `embed_srcs` | list&lt;string&gt; | `[]` | Files consumed by `//go:embed`, with Bazel and Buck2 spellings |
| `headers` | list&lt;string&gt; | `[]` | Buck2-compatible cgo header inputs |
| `header_namespace` | string | empty | Buck2-compatible logical header namespace retained during migration |
| `data`, `resources` | list&lt;string&gt; | `[]` | Runtime data globs and their Buck2-compatible spelling |
| `gc_goopts`, `compiler_flags` | list&lt;string&gt; | `[]` | Go compiler options using Bazel or Buck2 spelling |
| `assembler_flags` | list&lt;string&gt; | `[]` | Go assembler options |
| `tags`, `gotags`, `build_tags` | list&lt;string&gt; | `[]` | Go build constraint tags |
| `cgo` | bool | `false` | Enable cgo |
| `pure` | string | `auto` | Bazel-compatible cgo policy: `on`, `off`, or `auto` |
| `copts`, `cppopts`, `cxxopts`, `clinkopts` | list&lt;string&gt; | `[]` | Native compile, preprocessing, and link options propagated through cgo |
| `env` | map&lt;string, string&gt; | `{}` | Environment passed to Go build actions |
| `trimpath` | bool | `true` | Remove host paths from compiled artifacts |
| `race` | string | `auto` | Race detector mode: `auto`, `on`, or `off` |
| `msan`, `asan` | string | `auto` | Memory or address sanitizer mode: `auto`, `on`, or `off` |
| `pgoprofile` | string | empty | Package-relative profile for profile-guided optimization |
| `coverage`, `coverage_enabled` | bool | `false` | Enable coverage instrumentation |
| `coverage_mode` | string | empty | Coverage mode: `set`, `count`, or `atomic` |

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `go_package`, `go_dependency_set`, `c_provider` | Go packages, locked modules, and native providers |
| `embed` | `go_package` | Sources compiled into the same Go package |
| `cdeps` | `c_provider` | Native dependencies consumed through cgo |

## Providers and Capabilities

The target emits `go_package` and exposes `build` with no output group.
