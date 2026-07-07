# `zig_configure_test`

Once-native configured Zig test target.

## Description

Compiles a Zig test binary with `zig test --test-no-exec`, then runs that
binary through Once's generic test capability. Configuration is declared
directly on the target with attributes such as `mode`, `threaded`,
`zig_version`, and `zigopt`.

The root can be set with `main`, or omitted when `deps` contains exactly one
Zig module dependency. Test execution is host-only.

Canonical module names are generated from target labels with collision-safe
escaping. `import_names` keys must match exactly one Zig module dependency by
full label, short label name, import name, or canonical name; ambiguous or
unknown keys fail analysis.

## Attributes

This target accepts the complete attribute set documented by
[`zig_test`](/reference/prelude/zig_test), including `strip`,
`translate_c_identity`, `csrcs`, linker settings, `env_inherit`, `test_env`,
`test_runner`, `labels`, and `timeout_ms`.

## Dep Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `zig_module`, `c_provider` | Zig modules and C provider dependencies consumed by the test |

## Providers

The target emits `zig_test` and `once_test_info`.

## Capabilities

| Capability | Output groups | Requires |
| --- | --- | --- |
| `build` | `binary`, `asm`, `llvm_ir`, `llvm_bc`, `zig_docs` |  |
| `test` | `default`, `test_results`, `logs` | `binary` |

## Example

```toml
[[target]]
name = "math_tests"
kind = "zig_configure_test"
srcs = ["src/**/*.zig"]

[target.attrs]
main = "src/math_test.zig"
mode = "debug"
threaded = "multi"
labels = ["unit"]
```
