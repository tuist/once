# `bazel_target`

Resolver-generated [Bazel](https://bazel.build) library-style rule
materialized as a Once graph target.

## Description

`bazel_target` is created by the
[`bazel_workspace`](/reference/prelude/bazel_workspace) resolver for every
rule whose class does not end in `_test` or `_binary`. Direct authoring of
`bazel_target` in `once.toml` is rejected: the resolver must materialize it
so the `bazel_label` and `bazel_rule_kind` attributes stay tied to real
Bazel state.

`bazel_target` reads the action graph for its label through
`bazel aquery` and executes the selected default output, runfiles and validation
closure from `<workspace>/.once/bazel-actions/`. Bazel supplies analysis and
external repository downloads. `bazel build` is not invoked in the ownership
path. Owned spawns have explicit environments and declared inputs and outputs;
spawn caching additionally requires complete `toolchain_inputs`. File writes,
aliases and manifest-mapped runfiles use cacheable Once primitives.

Before a spawn executes on a cache miss, Once removes its declared outputs,
matching Bazel's clean-output contract. Warm hits restore those outputs instead
of rerunning the command.

Unknown required action contracts fall back to `bazel build <label>` and
record the unsupported mnemonics on the provider. Delegated commands remain
uncached by Once.

Ownership-mode outputs land under `<workspace>/.once/bazel-actions/bazel-out/`.
Fallback-mode outputs land under Bazel's `bazel-bin` and `bazel-out` as
usual.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `bazel_label` | string | yes |  | Fully qualified Bazel label of the underlying rule, for example `//src:kura_lib`. Set by the resolver. |
| `bazel_rule_kind` | string | yes |  | Bazel rule class reported by `bazel query --output=label_kind`, for example `rust_library`. Set by the resolver. |
| `bazel` | string | no | `bazel` | Bazel executable forwarded from the workspace seed. |
| `env` | map&lt;string,string&gt; | no | `{}` | Declared environment forwarded from the workspace seed. Empty values override inherited host variables; native `aquery` action environments take precedence. |
| `toolchain_inputs` | list&lt;string&gt; | no | `[]` | Complete implicit host toolchain inputs, forwarded from the workspace seed. Required for spawn caching. |
| `test_env` | map&lt;string,string&gt; | no | `{}` | Test runtime environment forwarded from the workspace seed. |

## Providers and capabilities

The target emits `bazel_target` and exposes only the `build` capability. Use
[`bazel_test`](/reference/prelude/bazel_test) for test rules and
[`bazel_binary`](/reference/prelude/bazel_binary) for binary rules.

## Direct use

`bazel_target` is discovered through its workspace seed. Query the graph to
list the materialized rules and their labels:

```sh
once query targets
```

Build one label without leaving the Once command surface:

```sh
once bazel build //src:kura_lib
```

See the [`bazel_workspace`](/reference/prelude/bazel_workspace) reference for
the workspace-level attributes that control graph enumeration.

## Sources

- [Bazel command-line reference](https://bazel.build/reference/command-line-reference)
  documents the `bazel build` invocation this target kind wraps.
