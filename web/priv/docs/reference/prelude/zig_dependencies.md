# `zig_dependencies`

Imports the locked package graph from `build.zig.zon` and exposes its direct
packages as one Zig dependency set. Zig remains authoritative for package
identity and content verification. Once creates ordinary synthetic targets
from already materialized source directories and does not fetch packages during
a build.

## Attributes

| Attribute | Type | Required | Default | Meaning |
| --- | --- | --- | --- | --- |
| `manifest` | string | no | `build.zig.zon` | Package-relative root Zig package manifest |
| `resolver_inputs` | list of strings | no | `srcs` | Package-relative text globs supplied to the resolver |
| `vendor_dir` | string | no | `third_party/zig` | Directory containing content-addressed package sources |
| `package_paths` | map of strings | no | empty | Overrides from dependency alias or content multihash to a source directory |
| `module_paths` | map of strings | no | empty | Overrides from dependency alias or content multihash to its public root module |

The resolver also maintains an internal root-package list. It is typed in the
schema so graph validation and query output remain complete.

## Providers

- `zig_dependency_set`

Zig binary, test, library, and configured targets accept this provider through
their normal `deps` field. The set exposes only packages directly declared by
the root manifest. Their transitive package edges remain attached to the
synthetic package targets.

## Example

```toml
[[target]]
name = "zig_dependencies"
kind = "zig_dependencies"
srcs = [
  "build.zig.zon",
  "third_party/zig/**/build.zig.zon",
]

[target.attrs]
resolver_inputs = ["build.zig.zon", "third_party/zig/**/build.zig.zon"]
vendor_dir = "third_party/zig"

[[target]]
name = "tool"
kind = "zig_binary"
srcs = ["src/**/*.zig"]
deps = ["zig_dependencies"]

[target.attrs]
main = "src/main.zig"
```

Remote packages should be materialized under the vendor directory using the
content multihash recorded in the manifest. Run Zig's native fetch workflow to
verify that hash before committing the materialized source. Once includes the
actual source contents in build action keys. Local path dependencies are
resolved relative to the manifest that declares them. Use `package_paths` when
the source layout differs and `module_paths` when the public module is not
`src/root.zig`.

Each generated package declares its complete materialized source tree as a
build input. This includes non-Zig assets read through `@embedFile` as well as
manifests and Zig sources.

Path dependency identity uses its normalized materialized source root, so the
same relative spelling in two parent packages cannot collapse distinct
packages. Resolution fails explicitly above 10,000 package instances.
