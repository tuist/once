# `zig_c_library`

Zig module generated from C provider headers.

## Description

Runs `zig translate-c` over headers from one or more `c_provider`
dependencies and exposes the generated Zig source as a `zig_module`.
The C provider link inputs are preserved so downstream Zig build targets can
compile against the translated declarations and link against the native
library inputs.

Set `translate_c_identity` when using a standalone `translate-c` path that can
change in place and the action cache should distinguish different executable
builds.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `zig` | string | no | `zig` on PATH | Zig compiler path |
| `zig_version` | string | no | empty | Expected `zig version` output when using `zig translate-c` |
| `import_name` | string | no | target name | Name downstream modules use with `@import` |
| `zigopts` | list&lt;string&gt; | no | `[]` | Extra flags attached to this module specification |
| `data` | list&lt;string&gt; | no | `[]` | Runtime data globs included in provider metadata |
| `target` | string | no | host target | Zig target triple passed to `zig translate-c` |
| `cpu` | string | no | empty | Processor setting passed with `-mcpu` |
| `mode` | string | no | empty | `auto`, `debug`, `release_safe`, `release_small`, or `release_fast`; maps to `-O` |
| `host_mode` | string | no | empty | Alias for `mode` when porting configured host targets |
| `threaded` | string | no | empty | `multi` passes `-fno-single-threaded`; `single` passes `-fsingle-threaded` |
| `host_threaded` | string | no | empty | Alias for `threaded` when porting configured host targets |
| `strip` | bool | no | `false` | Alias for `strip_debug_symbols` |
| `strip_debug_symbols` | bool | no | `false` | Pass `-fstrip` |
| `zigopt` | list&lt;string&gt; | no | `[]` | Global Zig flags prepended before module-specific `zigopts` |
| `host_zigopt` | list&lt;string&gt; | no | `[]` | Alias for `zigopt` when porting configured host targets |
| `use_cc_common_link` | int | no | `-1` | Compatibility setting for C provider linking; Once consumes C providers through its provider model |
| `host_use_cc_common_link` | int | no | `-1` | Host compatibility setting for C provider linking |
| `use_standalone_translate_c` | int | no | `-1` | Set to `1` to use a standalone `translate-c` executable |
| `translate_c` | string | no | empty | Standalone `translate-c` executable path |
| `translate_c_identity` | string | no | empty | Stable identity for the standalone `translate-c` executable, folded into translate action cache keys |
| `bootstrapped` | int | no | `-1` | Compatibility selector for bootstrapped Zig toolchains; choose the compiler with `zig` |

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `c_provider` | C provider dependencies whose headers are translated and whose link inputs are propagated |

## Providers

The target emits `zig_module`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `source` |

## Example

```toml
[[target]]
name = "native_math_zig"
kind = "zig_c_library"
deps = ["./native_math"]

[target.attrs]
import_name = "native_math"
```
