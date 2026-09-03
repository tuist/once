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

Both capabilities read the action graph for the label through `bazel aquery`
and run every action themselves from
`<workspace>/.once/bazel-shadow/<target>/`. Bazel handles analysis and
external-repository download via `bazel fetch`; Once handles execution.
When the action graph contains a Bazel-internal action (with no argv in
`aquery` output), the capability falls back to `bazel run <label>` (or
`bazel build <label>` for the `build` capability) and records the
unsupported mnemonics on the target's provider.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `bazel_label` | string | yes |  | Fully qualified Bazel label of the binary rule, for example `//src:kura`. Set by the resolver. |
| `bazel_rule_kind` | string | yes |  | Bazel rule class, for example `rust_binary`. Set by the resolver. |
| `bazel` | string | no | `bazel` | Bazel executable forwarded from the workspace seed. |

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
