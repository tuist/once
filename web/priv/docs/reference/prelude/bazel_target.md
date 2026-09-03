# `bazel_target`

Resolver-generated [Bazel](https://bazel.build) rule materialized as a Once
graph target.

## Description

`bazel_target` is created by the
[`bazel_workspace`](/reference/prelude/bazel_workspace) resolver. Every rule
returned by the workspace query becomes one `bazel_target` in the graph.
Direct authoring of `bazel_target` in `once.toml` is rejected: the resolver
must materialize it so the `bazel_label` and `bazel_rule_kind` attributes
stay tied to real Bazel state.

In this first integration each capability delegates execution to Bazel. Rule
kinds that Once learns to compile directly (rules_rs targets to Once's
[`rust_binary`](/reference/prelude/rust_binary) and
[`rust_library`](/reference/prelude/rust_library), rules_cc targets to
[`c_library`](/reference/prelude/c_library), and so on) will emit those
native kinds from the resolver instead. Rules Once has not lowered yet keep
using this delegating target kind.

The `build`, `test`, and `run` capabilities all shell out to Bazel:

- `build` calls `bazel build <label>`.
- `test` calls `bazel test <label> --test_output=errors`.
- `run` calls `bazel run <label>`.

The action inherits the caller's environment (`PATH`, `HOME`,
`SSL_CERT_FILE`, and other network- and toolchain-related variables) and
runs unsandboxed with network access so Bazel can reach its repository cache
and remote resources. The action is not cached at Once's level; Bazel manages
its own action cache, and layering another cache above it would double-count.

Every capability writes a small `<capability>.stamp` file under Once's output
tree. The stamp confirms that Bazel finished successfully so Once has an
observable artifact per capability, even though the real outputs live under
`bazel-bin` and `bazel-out` where Bazel keeps them.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `bazel_label` | string | yes |  | Fully qualified Bazel label of the underlying rule, for example `//src:kura`. Set by the resolver. |
| `bazel_rule_kind` | string | yes |  | Bazel rule class reported by `bazel query --output=label_kind`, for example `rust_binary`. Set by the resolver. |
| `bazel` | string | no | `bazel` | Bazel executable forwarded from the workspace seed. |

The resolver additionally sets `_bazel_capabilities` and `_bazel_resolved`.
Both are internal markers; they cannot be authored directly.

## Providers and capabilities

The target emits `bazel_target` and exposes `build`, `test`, and `run`.
Attempting `test` on a rule that is not a test, or `run` on a rule that is
not a binary, fails at Bazel invocation time.

## Direct use

`bazel_target` is discovered through its workspace seed. Query the graph
after `once native show bazel` to list the materialized rules and their
labels:

```sh
once query targets
```

Build, test, or run one label without leaving the Once command surface:

```sh
once bazel build //src:kura
once bazel test //src:kura_lib_test
```

See the [`bazel_workspace`](/reference/prelude/bazel_workspace) reference for
the workspace-level attributes that control graph enumeration.

## Sources

- [Bazel command-line reference](https://bazel.build/reference/command-line-reference)
  documents the `bazel build`, `bazel test`, and `bazel run` invocations
  this target kind wraps.
