---
prev: false
next: false
---

# Go

Once can build Go source groups, package archives, executables, exported C
libraries, and tests. It imports vendored modules as typed graph targets, keeps
ordinary builds offline, supports destination operating system and architecture
selection, and exposes normalized test results and coverage.

The target kinds adapt the public [Bazel rules_go](https://github.com/bazel-contrib/rules_go)
and [Buck2 Go prelude](https://github.com/facebook/buck2/tree/main/prelude/go)
behavior to Once actions. Once invokes the Go command directly. Source
selection, module inputs, generated outputs, cache directories, and test
artifacts stay visible to the action model.

## Prerequisites

Install the pinned Go toolchain through mise:

```sh
mise install
mise exec -- go version
```

## Declare a Module

Create `once.toml` beside `go.mod`:

```toml
[[target]]
name = "GoDependencies"
kind = "go_dependencies"
srcs = ["go.mod", "go.sum", "vendor/modules.txt"]

[target.attrs]
resolver_inputs = ["go.mod", "go.sum", "vendor/modules.txt"]

[[target]]
name = "Greeting"
kind = "go_library"
srcs = ["internal/greeting/*.go"]
deps = ["GoDependencies"]

[target.attrs]
package = "./internal/greeting"
importpath = "example.com/hello/internal/greeting"

[[target]]
name = "Hello"
kind = "go_binary"
srcs = ["cmd/hello/*.go"]
deps = ["Greeting", "GoDependencies"]

[target.attrs]
package = "./cmd/hello"

[[target]]
name = "GreetingTests"
kind = "go_test"
srcs = ["internal/greeting/*_test.go"]
deps = ["GoDependencies"]

[target.dependencies]
target_under_test = ["Greeting"]

[target.attrs]
package = "./internal/greeting"
coverage = true
labels = ["unit"]
```

`go_source` can group files that another target embeds into the same package.
Connect it through the `embed` dependency role. Native dependencies used by
cgo belong under the `cdeps` role.

## Import External Modules

Go remains authoritative for version selection and source verification. Update
and vendor dependencies before an ordinary Once build:

```sh
go mod tidy
go mod vendor
```

`go_dependencies` reads the complete requirement graph from a Go 1.17 or newer
`go.mod`, binds selected versions to source checksums from `go.sum`, and checks
those selections against `vendor/modules.txt`. It emits one `go_module` target
for every vendored module that contributes a package. Each generated target
carries its exact module path, version, source checksum, package list, and
complete vendored source tree. The Go vendoring workflow verifies source before
it is committed, while Once hashes the committed tree into every consuming
action.

Build and test actions use `-mod=vendor`, `GOPROXY=off`, and `GOSUMDB=off`.
They cannot fetch missing source. A missing checksum, a version disagreement,
or a vendored module absent from the complete requirement graph fails while the
graph loads.

For a `go.work` workspace, set `workspace_file`, include that file and every
relevant checksum file in `sum_files` and `resolver_inputs`, and create the
vendor tree with `go work vendor`. Consumers set `go_work` to the same
workspace-relative file.

## Query, Build, Run, and Test

Inspect the graph before executing it:

```sh
once query target-kinds
once query schema go_binary
once query targets --kind go_module
once query capabilities Hello
```

Then use the shared capability commands:

```sh
once build Hello
once run Hello
once test GreetingTests
```

The test target compiles one native Go test binary, runs it through `go tool
test2json`, and writes [JavaScript Object Notation](https://www.json.org/json-en.html)
events, a readable log, and `once.test_results.v1` output. Exact test names from
`once query test-manifest` can be scheduled independently. Coverage writes a
native Go coverage profile alongside those results.

## Cross-Compile and Export Native Libraries

Set `goos` and `goarch` on a binary or library to select its destination:

```toml
[[target]]
name = "HelloLinux"
kind = "go_binary"
srcs = ["cmd/hello/*.go"]
deps = ["Greeting", "GoDependencies"]

[target.attrs]
package = "./cmd/hello"
goos = "linux"
goarch = "amd64"
strip = true
```

`tags`, `gotags`, and `build_tags` accept the corresponding Bazel and Buck2
spellings. Compiler, assembler, linker, race detector, memory sanitizer,
address sanitizer, profile-guided optimization, stripping, and link-time string
definitions are also available through typed attributes.

Set `build_mode` to `c-archive` or `c-shared` to expose a Go `main` package to
native consumers. Once returns the generated header and library through its
generic C provider, including the Android
[Application Binary Interface](https://developer.android.com/ndk/guides/abis)
record when `android_abi` is set. Other supported build modes are `exe`, `pie`,
`plugin`, `shared`, and `archive`.

Cross-compiled executables can be built on the host. `once run` and `once test`
require a binary for the host operating system and architecture. Cross-platform
cgo also requires destination-compatible `cc` and `c++` commands.

## Runnable Starter

The `go-comprehensive` starter includes a first-party package, an embedded
source group, a runnable executable, a Linux cross-compiled executable, exact
tests with coverage, and the vendored `github.com/pkg/errors` module:

```sh
once query example go_binary go-comprehensive --format json
```
