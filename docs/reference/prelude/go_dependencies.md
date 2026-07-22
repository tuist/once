# `go_dependencies`

Imports a locked, vendored Go module graph.

## Description

The resolver reads `go.mod` or a Go workspace, declared checksum files, and
`vendor/modules.txt`. It creates one queryable `go_module` target per selected
external module. Ordinary build actions use the vendor tree with network
module lookup disabled.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `manifest` | string | no | `go.mod` | Package-relative module manifest |
| `workspace_file` | string | no | empty | Package-relative `go.work` file for a multi-module workspace |
| `sum_files` | list&lt;string&gt; | no | `["go.sum"]` | Checksum files authenticating selected module sources |
| `vendor_dir` | string | no | `vendor` | Package-relative directory produced by `go mod vendor` or `go work vendor` |
| `resolver_inputs` | list&lt;string&gt; | no | `srcs` | Manifest, checksum, workspace, and vendor manifest files supplied to the resolver |
| `_go_resolved` | bool | resolver-owned | `false` | Marks a dependency set expanded by the resolver |
| `_go_module_targets` | list&lt;string&gt; | resolver-owned | `[]` | Names of generated module targets |

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `go_module` | Generated locked modules aggregated by this dependency set |

## Providers and Capabilities

The target emits `go_dependency_set` and exposes `build` with no output group.

## Example

```toml
[[target]]
name = "GoDependencies"
kind = "go_dependencies"
srcs = ["go.mod", "go.sum", "vendor/modules.txt"]

[target.attrs]
resolver_inputs = ["go.mod", "go.sum", "vendor/modules.txt"]
```

See the [Go guide](/guide/graph/go#import-external-modules) for the update and
vendoring workflow.

