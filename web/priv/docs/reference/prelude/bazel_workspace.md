# `bazel_workspace`

Native Bazel workspace seed.

## Description

Once supplies this seed automatically when it finds `MODULE.bazel`,
`WORKSPACE.bazel`, or `WORKSPACE`. The resolver exposes one complete-workspace
command target. Bazel remains responsible for evaluating its graph, resolving
repositories, selecting toolchains, and maintaining its incremental cache.

No `once.toml` is required or created. A Bazel root owns nested native project
markers so examples stored below it do not become competing default roots.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `resolver_inputs` | list&lt;string&gt; | no | `[]` | Bazel module, workspace, build, and extension files available during resolution |

## Providers

The target emits `bazel_workspace`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | none |

Building the seed invokes `bazel build //...` through the generated
[`bazel_command`](/reference/prelude/bazel_command) target. Targetless
`once test` invokes `bazel test //...` through the same target.

## Example

```sh
once query targets
once build --ui
once test
```

See [Bazel](/guide/graph/bazel) for prerequisites and current boundaries.
