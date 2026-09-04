# `bazel_test`

Resolver-generated [Bazel](https://bazel.build) test rule materialized as a
Once graph target.

## Description

`bazel_test` is created by the
[`bazel_workspace`](/reference/prelude/bazel_workspace) resolver for every
rule whose class ends in `_test` (for example `rust_test`, `cc_test`,
`py_test`). Direct authoring in `once.toml` is rejected: the resolver must
materialize it so the `bazel_label` and `bazel_rule_kind` attributes stay
tied to real Bazel state.

Both capabilities read the action graph for the label through `bazel aquery`
and run every action themselves from
`<workspace>/.once/bazel-shadow/<target>/`. Bazel handles analysis and
external-repository download via `bazel fetch`; Once handles execution.
When the action graph contains a Bazel-internal action (with no argv in
`aquery` output), the capability falls back to `bazel test <label>
--test_output=errors` (or `bazel build <label>` for the `build` capability)
and records the unsupported mnemonics on the target's provider.

Test rules are not roots of the resolver output: `once build` or
`once bazel build //...` never picks them up implicitly. Invoke the test
explicitly with `once bazel test` or `once test <once-id>`.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `bazel_label` | string | yes |  | Fully qualified Bazel label of the test rule, for example `//src:kura_lib_test`. Set by the resolver. |
| `bazel_rule_kind` | string | yes |  | Bazel rule class, for example `rust_test`. Set by the resolver. |
| `bazel` | string | no | `bazel` | Bazel executable forwarded from the workspace seed. |

## Providers and capabilities

The target emits `bazel_test` and `bazel_target` and exposes the `build` and
`test` capabilities. It does not expose `run`.

## Direct use

```sh
once bazel test //src:kura_lib_test
```

See the [`bazel_workspace`](/reference/prelude/bazel_workspace) reference for
the workspace-level attributes that control graph enumeration.

## Sources

- [Bazel command-line reference](https://bazel.build/reference/command-line-reference)
  documents `bazel test`, which this target kind wraps.
