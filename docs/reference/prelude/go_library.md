# `go_library`

Go package compiled into an archive.

The target accepts every [common Go attribute](/reference/prelude/go_source#common-go-attributes),
including source embedding, build constraints, cgo, cross-compilation,
sanitizers, coverage, and compiler options. It additionally accepts `x_defs`
as a map of Bazel-compatible link-time string definitions retained for
embedded binary migrations.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `go_package`, `go_dependency_set`, `c_provider` | Go packages, locked modules, and native providers |
| `embed` | `go_package` | Sources compiled into the same package |
| `cdeps` | `c_provider` | Native dependencies consumed through cgo |

## Providers, Capabilities, and Outputs

The target emits `go_package`. `build` exposes the `library` output group and
writes `.once/out/<target>/lib<target>.a`, or the destination platform archive
extension.

## Example

```toml
[[target]]
name = "Greeting"
kind = "go_library"
srcs = ["internal/greeting/*.go"]
deps = ["GoDependencies"]

[target.attrs]
package = "./internal/greeting"
importpath = "example.com/hello/internal/greeting"
```

