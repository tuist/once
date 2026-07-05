# `zig_configure`

Once-native configured Zig library target.

## Description

Compiles a Zig library with configuration declared directly on the target. Set
`output = "static"` for `zig build-lib`, or `output = "shared"` for
`zig build-lib -dynamic`.

The root can be set with `main`, or omitted when `deps` contains exactly one
Zig module dependency. C provider deps contribute headers, definitions, include
directories, library inputs, and linker flags.

Canonical module names are generated from target labels with collision-safe
escaping. `import_names` keys must match exactly one Zig module dependency by
full label, short label name, import name, or canonical name; ambiguous or
unknown keys fail analysis.

## Attributes

This target accepts the complete build configuration attribute set documented by
[`zig_static_library`](/reference/prelude/zig_static_library), including
`import_name`, `import_names`, `strip`, `translate_c_identity`, `csrcs`, linker
settings, and auxiliary output settings. It also accepts:

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `output` | string | no | `static` | `static` or `shared` |
| `emit_bin` | bool | no | `true` | Emit the static library when `output` is `static` |
| `shared_lib_name` | string | no | platform name | Exact shared library output file name when `output` is `shared` |
| `android_abi` | string | no | empty | Android [Application Binary Interface](https://developer.android.com/ndk/guides/abis) directory for shared library packaging |

## Dep Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `zig_module`, `c_provider` | Zig modules and C provider dependencies consumed by the library |

## Providers

The target emits the same native provider fields as `zig_static_library` or
`zig_shared_library`, depending on `output`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `static_library`, `shared_library`, `asm`, `llvm_ir`, `llvm_bc`, `zig_docs` |

## Example

```toml
[[target]]
name = "math_release"
kind = "zig_configure"
deps = ["./math"]

[target.attrs]
mode = "release_fast"
threaded = "single"
output = "static"
```
