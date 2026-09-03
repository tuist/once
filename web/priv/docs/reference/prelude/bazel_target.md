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

In this first integration the `build` capability delegates execution to
Bazel. Rule kinds that Once learns to compile directly (rules_rs targets to
Once's [`rust_library`](/reference/prelude/rust_library), rules_cc targets to
[`c_library`](/reference/prelude/c_library), and so on) will emit those
native kinds from the resolver instead. Rules Once has not lowered yet keep
using this delegating target kind.

`bazel_target` calls `bazel build <label>` and inherits the caller's
environment (`PATH`, `HOME`, `SSL_CERT_FILE`, and other network- and
toolchain-related variables) so Bazel can reach its repository cache and
remote resources. The action is not cached at Once's level; Bazel manages
its own action cache, and layering another cache above it would double-count.

Real outputs live under `bazel-bin` and `bazel-out` where Bazel keeps them.

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

`bazel_target` is discovered through its workspace seed. Query the graph
after `once native show bazel` to list the materialized rules and their
labels:

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
