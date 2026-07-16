# `zig_library`

Zig module provider.

## Description

Defines a Zig module with a root source file and optional Zig or C provider
dependencies. The target does not compile by itself. Downstream Zig binary,
test, static library, and shared library targets compile modules at the use
site so Zig can perform whole-program compilation.

If the module depends on a C provider, downstream Zig builds receive a generated
`c` module from `zig translate-c`, plus the C compile and link inputs.

Canonical module names are generated from target labels with collision-safe
escaping. `import_names` keys must match exactly one Zig module dependency by
full label, short label name, import name, or canonical name; ambiguous or
unknown keys fail validation.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `main` | string | yes |  | Package-relative root Zig module |
| `import_name` | string | no | target name | Name downstream modules use with `@import` |
| `import_names` | map&lt;string, string&gt; | no | `{}` | Rename dependency imports by dependency label, short name, or import name |
| `extra_srcs` | list&lt;string&gt; | no | `[]` | Extra source or data globs included in dependent compile input digests |
| `data` | list&lt;string&gt; | no | `[]` | Runtime data globs propagated to dependents |
| `zigopts` | list&lt;string&gt; | no | `[]` | Extra flags attached to this module specification |

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `zig_module`, `c_provider` | Zig modules and C provider dependencies consumed by this module |

## Providers

The target emits `zig_module`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | none |

## Example

```toml
[[target]]
name = "math"
kind = "zig_library"
srcs = ["src/**/*.zig"]

[target.attrs]
main = "src/math.zig"
import_name = "math"
```
