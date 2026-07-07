# `zig_configure_binary`

Once-native configured Zig executable target.

## Description

Compiles a Zig executable with `zig build-exe`, with configuration declared
directly on the target. This is the Once shape for configured Zig binaries:
use target attributes such as `mode`, `threaded`, `zig_version`, and `zigopt`
instead of wrapping another target through a transition.

The root can be set with `main`, or omitted when `deps` contains exactly one
Zig module dependency.

Canonical module names are generated from target labels with collision-safe
escaping. `import_names` keys must match exactly one Zig module dependency by
full label, short label name, import name, or canonical name; ambiguous or
unknown keys fail analysis.

## Attributes

This target accepts the complete attribute set documented by
[`zig_binary`](/reference/prelude/zig_binary), including `strip`,
`translate_c_identity`, `csrcs`, linker settings, run `env`, and run `args`.

## Dep Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `zig_module`, `c_provider` | Zig modules and C provider dependencies consumed by the executable |

## Providers

The target emits `zig_binary`.

## Capabilities

| Capability | Output groups | Requires |
| --- | --- | --- |
| `build` | `binary`, `asm`, `llvm_ir`, `llvm_bc`, `zig_docs` |  |
| `run` | `default` | `binary` |

## Example

```toml
[[target]]
name = "hello_release"
kind = "zig_configure_binary"
deps = ["./math"]
srcs = ["src/**/*.zig"]

[target.attrs]
main = "src/main.zig"
mode = "release_safe"
zigopt = ["-fllvm"]
```
