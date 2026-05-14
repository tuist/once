# Go

Fabrik supports dependency graph sync through `fabrik deps sync` and a
cacheable `go.binary` rule that delegates module resolution to the Go
toolchain.

## Dependency Sync

Declare Go dependencies in the root `fabrik.toml` and point the manifest
at the module's `go.mod`.

```toml
[[deps]]
name = "go"
ecosystem = "go"
manifest = "go.mod"
output = "vendor/fabrik.go.lock.json"
```

Run it:

```sh
fabrik deps sync go
```

The Go sync step runs `go list -m -json all` from the directory that
contains the declared `go.mod`. It emits a lock graph JSON file with
module paths, versions, sums, replacements, and local path sources. The
`go list` resolution action is cached from the module files in the
workspace.

## Build Targets

Use `go.binary` for command packages. The action runs `go build` from
the directory containing the target's `fabrik.toml`, so `go.mod`,
`go.sum`, `replace`, and workspace behavior stay native to Go.

```toml
[[go.binary]]
name = "server"
srcs = ["go.mod", "go.sum", "main.go"]
deps = [{ go = "github.com/acme/lib/subpkg" }]
```

The inline table key points to the named `[[deps]]` graph. The value is
interpreted by the Go adapter as the module package imported by the
target, and is included in the action cache key. The actual module
selection remains owned by `go build`.

## Current Limits

- `go.binary` delegates the full package graph to `go build`; Fabrik
  does not split Go packages into separate granular actions yet.
- Local Fabrik target deps on `go.binary` are not wired yet.
- Go module resolution remains owned by the Go toolchain.
