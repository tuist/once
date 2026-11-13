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

Both capabilities select the default output, runfiles and validation closure
from Bazel's analysis and execute it in `<workspace>/.once/bazel-actions/`.
Build skips test execution. Test uses the exposed launcher and runfiles
manifest with environment, timeout and sharding. Bazel's exposed argv already
contains expanded rule arguments; Once forwards them once without appending the
unexpanded rule attribute. GNU `timeout` or `gtimeout` must be available on the
host. Test execution is intentionally uncached, while compilation and runfiles
preparation remain cacheable with the workspace's toolchain declarations.
Test command options from `.bazelrc` are not inferred; declare required runtime
values in `test_env`.

Opaque required actions or unsupported test action shapes delegate to
`bazel test <label> --test_output=errors` (or `bazel build <label>` for build).
The provider reports the unsupported mnemonics. Delegated commands remain
uncached by Once. Flaky retries and general configured rule-attribute expansion
are not implemented by the owned test strategy.

Test rules are not roots of the resolver output: `once build` or
`once bazel build //...` never picks them up implicitly. Invoke the test
explicitly with `once bazel test` or `once test <once-id>`.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `bazel_label` | string | yes |  | Fully qualified Bazel label of the test rule, for example `//src:kura_lib_test`. Set by the resolver. |
| `bazel_rule_kind` | string | yes |  | Bazel rule class, for example `rust_test`. Set by the resolver. |
| `bazel` | string | no | `bazel` | Bazel executable forwarded from the workspace seed. |
| `env` | map&lt;string,string&gt; | no | `{}` | Declared environment forwarded from the workspace seed. Empty values override inherited host variables; native `aquery` action environments take precedence. |
| `toolchain_inputs` | list&lt;string&gt; | no | `[]` | Complete implicit host toolchain inputs, forwarded from the workspace seed. Required for spawn caching. |
| `test_env` | map&lt;string,string&gt; | no | `{}` | Additional declared test runtime environment, overriding rule environment. |

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
