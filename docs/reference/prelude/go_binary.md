# `go_binary`

Buildable and runnable Go executable or exported native library.

The target accepts every [common Go attribute](/reference/prelude/go_source#common-go-attributes)
plus the link and run attributes below.

## Attributes

| Attribute | Type | Default | Description |
| --- | --- | --- | --- |
| `out` | string | empty | Exact output file name |
| `basename`, `output_name` | string | target name | Bazel and Buck2 output base-name spellings |
| `build_mode` | string | `exe` | `exe`, `pie`, `plugin`, `c-archive`, `c-shared`, `shared`, or `archive` |
| `linkmode` | string | `auto` | Bazel-compatible build mode alias |
| `link_style` | string | `static` | Buck2-compatible `static`, `static_pic`, or `shared` style |
| `link_mode` | string | `auto` | Buck2-compatible Go linker mode: `auto`, `internal`, or `external` |
| `gc_linkopts`, `linker_flags` | list&lt;string&gt; | `[]` | Go linker options |
| `external_linker_flags` | list&lt;string&gt; | `[]` | Options passed to the external linker |
| `x_defs` | map&lt;string, string&gt; | `{}` | Link-time string definitions lowered to `-X` |
| `strip` | bool | `false` | Strip symbol and debugging tables |
| `static` | string | `auto` | Static linking policy: `auto`, `on`, or `off` |
| `android_abi` | string | inferred | Android [Application Binary Interface](https://developer.android.com/ndk/guides/abis) directory for C shared outputs |
| `generate_exported_header` | bool | `false` | Buck2-compatible request for the header emitted by C archive and C shared modes |
| `args` | list&lt;string&gt; | `[]` | Arguments passed by `once run` |
| `run_env` | map&lt;string, string&gt; | `{}` | Runtime environment values |
| `env_inherit` | list&lt;string&gt; | `[]` | Host environment names inherited before `run_env` overrides |

The dependency roles match [`go_source`](/reference/prelude/go_source#dependency-edges).

## Providers

The target emits Go package and binary providers. `c-archive` and `c-shared`
also emit the generic C and native-linkable providers, including the generated
header and transitive native metadata.

## Capabilities and Outputs

| Capability | Output groups | Requires |
| --- | --- | --- |
| `build` | `binary` | none |
| `run` | `default` | `binary` |

Only `exe` and `pie` can run. Other build modes remain buildable outputs.
`once run` writes its log and result marker under `.once/out/<target>/run/`.

## Starter

The `go-binary-minimal` starter contains one executable and no external
dependencies. Discover its descriptor through `once query schema go_binary`,
then create it with `once edit materialize-example go_binary
go-binary-minimal`.
