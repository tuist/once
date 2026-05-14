# Go

Fabrik does not have granular Go build targets yet. The current Go
support is dependency graph sync through `fabrik deps sync`.

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
module paths, versions, sums, replacements, and local path sources.
Future Go targets will use the same target-side dependency edge shape
as other ecosystems:

```toml
deps = [
  "libs/common/common",
  { go = "github.com/acme/lib/subpkg" },
]
```

The inline table key points to the named `[[deps]]` graph. The value is
interpreted by the Go adapter as the module package imported by the
target.

## Current Limits

- Go dependency sync only produces dependency graph metadata today.
- Go target declarations can preserve `{ go = ... }` external
  dependency edges once Go rules exist.
- Fabrik does not yet generate Go build targets or invoke the Go
  compiler through cacheable granular actions.
- Go module resolution remains owned by the Go toolchain.
