# `zig_shared_library`

Zig shared library target.

## Description

Compiles a Zig shared library with `zig build-lib -dynamic`. The root can be
set with `main`, or omitted when `deps` contains exactly one Zig module
dependency. The output is exposed as a C provider and, when `android_abi` is
set, as an Android native-library provider.

The target also declares a documentation action that emits the `zig_docs`
output group.

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
| `data` | list&lt;string&gt; | no | `[]` | Runtime data globs propagated to dependents |
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
| `shared_lib_name` | string | no | platform name | Exact shared library output file name |
| `android_abi` | string | no | empty | Android [Application Binary Interface](https://developer.android.com/ndk/guides/abis) directory for packaging |
| `output_name` | string | no | target name | Output file name without platform extension |

## Dep Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `zig_module`, `c_provider` | Zig modules and C provider dependencies consumed by the library |

## Providers

The target emits `zig_shared_library`, `c_provider`, `native_linkable`, and
`android_native_library`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `shared_library`, `asm`, `llvm_ir`, `llvm_bc`, `zig_docs` |

## Example

```toml
[[target]]
name = "math"
kind = "zig_shared_library"
srcs = ["src/**/*.zig"]

[target.attrs]
main = "src/math.zig"
```
