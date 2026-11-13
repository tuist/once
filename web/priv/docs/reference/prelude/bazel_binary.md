# `bazel_binary`

Resolver-generated [Bazel](https://bazel.build) binary rule materialized as
a Once graph target.

## Description

`bazel_binary` is created by the
[`bazel_workspace`](/reference/prelude/bazel_workspace) resolver for every
rule whose class ends in `_binary` (for example `rust_binary`, `cc_binary`,
`py_binary`, `go_binary`). Direct authoring in `once.toml` is rejected: the
resolver must materialize it so the `bazel_label` and `bazel_rule_kind`
attributes stay tied to real Bazel state.

Build selects the default output, runfiles and validation closure from
Bazel's analysis and executes it in `<workspace>/.once/bazel-actions/`.
Spawns have declared inputs, outputs and environments. Spawn caching requires
complete `toolchain_inputs` on the workspace seed. File writes, aliases and
manifest-mapped runfiles use cacheable Once primitives. Unknown required
action contracts delegate to Bazel and are recorded on the provider;
delegated commands remain uncached by Once.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `bazel_label` | string | yes |  | Fully qualified Bazel label of the binary rule, for example `//src:kura`. Set by the resolver. |
| `bazel_rule_kind` | string | yes |  | Bazel rule class, for example `rust_binary`. Set by the resolver. |
| `bazel` | string | no | `bazel` | Bazel executable forwarded from the workspace seed. |
| `env` | map&lt;string,string&gt; | no | `{}` | Declared environment forwarded from the workspace seed. Empty values override inherited host variables; native `aquery` action environments take precedence. |
| `toolchain_inputs` | list&lt;string&gt; | no | `[]` | Complete implicit host toolchain inputs, forwarded from the workspace seed. Required for spawn caching. |
| `test_env` | map&lt;string,string&gt; | no | `{}` | Test runtime environment forwarded from the workspace seed. |

## Providers and capabilities

The target emits `bazel_binary` and `bazel_target` and exposes the `build`
and `run` capabilities. It does not expose `test`.

## Direct use

```sh
once bazel build //src:kura
```

Use `once run` when Once should invoke the compiled binary through the
graph after the build capability completes.

See the [`bazel_workspace`](/reference/prelude/bazel_workspace) reference for
the workspace-level attributes that control graph enumeration.

## Sources

- [Bazel command-line reference](https://bazel.build/reference/command-line-reference)
  documents `bazel build` and `bazel run`, which this target kind wraps.
