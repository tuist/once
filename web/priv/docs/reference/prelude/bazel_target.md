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
`bazel aquery` and runs every action itself from
`<workspace>/.once/bazel-shadow/<target>/`. Bazel does the analysis phase
(target loading, toolchain resolution, external repository download via
`bazel fetch`); Once does the execution phase. `bazel build` is not
invoked in the ownership path.

Actions inherit the caller's environment (`PATH`, `HOME`, `SSL_CERT_FILE`,
and other toolchain-related variables). When the action graph contains any
Bazel-internal action (`Symlink`, `FileWrite`, `RunfilesTree`,
`SymlinkTree`, `RepoMappingManifest`, which are actions with no argument vector in `aquery`
output), Once falls back to `bazel build <label>` for that target and
records the unsupported mnemonics on the target's provider.

Ownership-mode outputs land under `<workspace>/.once/bazel-shadow/<target>/bazel-out/`.
Fallback-mode outputs land under Bazel's `bazel-bin` and `bazel-out` as
usual.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `bazel_label` | string | yes |  | Fully qualified Bazel label of the underlying rule, for example `//src:kura_lib`. Set by the resolver. |
| `bazel_rule_kind` | string | yes |  | Bazel rule class reported by `bazel query --output=label_kind`, for example `rust_library`. Set by the resolver. |
| `bazel` | string | no | `bazel` | Bazel executable forwarded from the workspace seed. |

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
