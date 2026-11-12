# `zig_generated_library`

Generates a Zig module by running a declared host executable. The resulting
`zig_module` can be imported by ordinary Zig libraries, binaries, and tests.
This models build-time code generation without wrapping a native build system.

Declare exactly one `once_executable` dependency in the `generator` role.
Use `srcs` to declare every file the generator reads. Arguments are passed
directly, without a shell, from the workspace root. At least one argument must
contain `{output}`, which is replaced with the declared generated module path.
Generators must produce deterministic bytes from their declared inputs and
explicit environment. Their executable and runtime files are action inputs.

Module dependencies belong in `deps`, not `generator`. They are available when
the generated module is compiled at its downstream use site. Generator sources
do not become sources of the generated module.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `args` | list&lt;string&gt; | yes | | Generator arguments, including `{output}` |
| `output_name` | string | no | `generated.zig` | Generated file name inside the target output directory |
| `env` | map&lt;string, string&gt; | no | `{}` | Explicit generator environment |

The module attributes `import_name`, `import_names`, `extra_srcs`, `data`, and
`zigopts` have the same meanings as on [`zig_library`](/reference/prelude/zig_library).

## Example

```toml
[[target]]
name = "types"
kind = "zig_generated_library"
srcs = ["schema.json"]
deps = ["parser"]

[target.dependencies]
generator = ["codegen"]

[target.attrs]
args = ["--schema", "schema.json", "--output", "{output}"]
import_name = "types"
```
