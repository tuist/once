# `zig_test`

Zig test target.

## Description

Compiles a Zig test binary with `zig test --test-no-exec`, then runs that
binary through Once's generic test capability. Once writes a normalized test
result file with one suite-level case and stores the native Zig test output as
a log artifact.

The root can be set with `main`, or omitted when `deps` contains exactly one
Zig module dependency. Test execution is host-only. Cross-target test binaries
can still be built, but running them needs a platform runner outside this
target kind.

Canonical module names are generated from target labels with collision-safe
escaping. `import_names` keys must match exactly one Zig module dependency by
full label, short label name, import name, or canonical name; ambiguous or
unknown keys fail analysis.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `zig` | string | no | `zig` on PATH | Zig compiler path |
| `zig_version` | string | no | empty | Expected `zig version` output |
| `bootstrapped` | int | no | `-1` | Compatibility selector for bootstrapped Zig toolchains; choose the compiler with `zig` |
| `main` | string | no |  | Package-relative root module, or omit with one Zig module dependency |
| `import_name` | string | no | target name | Name downstream modules use with `@import` |
| `import_names` | map&lt;string, string&gt; | no | `{}` | Rename dependency imports by dependency label, short name, or import name |
| `extra_srcs` | list&lt;string&gt; | no | `[]` | Extra source or data globs included in the compile input digest |
| `extra_docs` | list&lt;string&gt; | no | `[]` | Extra files included in the documentation action input digest |
| `data` | list&lt;string&gt; | no | `[]` | Runtime data globs included in test run inputs |
| `target` | string | no | host target | Zig target triple passed with `-target` |
| `cpu` | string | no | empty | Processor setting passed with `-mcpu` |
| `optimize` | string | no | empty | Optimization mode passed with `-O` |
| `mode` | string | no | empty | `auto`, `debug`, `release_safe`, `release_small`, or `release_fast`; maps to `-O` |
| `host_mode` | string | no | empty | Alias for `mode` when porting configured host targets |
| `threaded` | string | no | empty | `multi` passes `-fno-single-threaded`; `single` passes `-fsingle-threaded` |
| `host_threaded` | string | no | empty | Alias for `threaded` when porting configured host targets |
| `strip` | bool | no | `false` | Alias for `strip_debug_symbols` |
| `strip_debug_symbols` | bool | no | `false` | Pass `-fstrip` |
| `compiler_runtime` | string | no | `default` | `default`, `include`, or `exclude` for compiler runtime flags |
| `zigopt` | list&lt;string&gt; | no | `[]` | Global Zig flags prepended before module-specific `zigopts` |
| `host_zigopt` | list&lt;string&gt; | no | `[]` | Alias for `zigopt` when porting configured host targets |
| `zigopts` | list&lt;string&gt; | no | `[]` | Extra flags attached to this module specification |
| `linkopts` | list&lt;string&gt; | no | `[]` | Extra linker flags appended after Once-managed flags |
| `use_cc_common_link` | int | no | `-1` | Compatibility setting for C provider linking; Once consumes C providers through its provider model |
| `host_use_cc_common_link` | int | no | `-1` | Host compatibility setting for C provider linking |
| `use_standalone_translate_c` | int | no | `-1` | Set to `1` to use a standalone `translate-c` executable |
| `translate_c` | string | no | empty | Standalone `translate-c` executable path |
| `translate_c_identity` | string | no | empty | Stable identity for the standalone `translate-c` executable, folded into translate action cache keys |
| `csrcs` | list&lt;string&gt; | no | `[]` | C source files passed to Zig through `-cflags ... --` |
| `copts` | list&lt;string&gt; | no | `[]` | C compiler flags passed after `-cflags` |
| `linker_script` | string | no | empty | Package-relative linker script passed as `-T` |
| `emit_asm` | bool | no | `false` | Emit assembly output |
| `emit_llvm_ir` | bool | no | `false` | Emit compiler intermediate representation output |
| `emit_llvm_bc` | bool | no | `false` | Emit compiler bitcode output |
| `output_name` | string | no | target name | Output file name without platform extension |
| `env` | map&lt;string, string&gt; | no | `{}` | Environment variables passed to the Zig test binary |
| `env_inherit` | list&lt;string&gt; | no | `[]` | Host environment variable names inherited by test execution |
| `args` | list&lt;string&gt; | no | `[]` | Arguments passed to the compiled Zig test binary |
| `test_env` | map&lt;string, string&gt; | no | `{}` | Once-compatible alias for extra test execution environment variables |
| `test_runner` | string | no | empty | Package-relative Zig file passed as `--test-runner` |
| `labels` | list&lt;string&gt; | no | `[]` | Labels exposed through `once_test_info` for test discovery |
| `timeout_ms` | int | no |  | Optional test timeout in milliseconds |

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
kind = "zig_test"
srcs = ["src/**/*.zig"]

[target.attrs]
main = "src/math_test.zig"
labels = ["unit"]
```
