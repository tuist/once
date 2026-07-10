# `elixir_library`

Elixir code compiled into cacheable bytecode and OTP application metadata.

## Description

`elixir_library` runs one `elixirc` action for the full target, then stages the
compiled bytecode and generated OTP application metadata as Once build outputs.
Downstream Elixir targets receive the compiled OTP application on the code path,
so tests and dependent libraries do not compile the same project again.

The compile action passes all target sources to Elixir together, so Elixir can
resolve same-target compile-time relationships such as macros, structs, and
protocols while using its own parallel compiler internally. The staging action
depends on the compile action and rewrites the application metadata after any
compiled module changes.

Once records Elixir compile metadata alongside bytecode. That metadata tracks
external resources, compile environment reads, modules that export
`__mix_recompile__?/0`, protocols, implementations, and compiler warnings. On
later builds, Once runs a small stale-check action before compilation so changes
to dynamic inputs can invalidate the cached compile action even when the source
files themselves did not change.

Compile-time dependencies that must be loaded while compiling, such as macros
or structs used by another target, should be modeled as separate
`elixir_library` dependencies. Config and data globs are available to the
action and are included in cache keys. Config files are loaded before
compilation using Elixir's config reader so `Application.compile_env` sees the
same values the target records in compile metadata. Once does not run Mix's
compile task or require a Mix project during library compilation.

Library compilation does not require a Mix project file by default. Set
`mix_config` only when a project file should participate in cache keys.
Dependencies should come from `deps`, not from precompiled build directories
passed through `compile_args`; otherwise Once cannot cache or rebuild them at
target granularity.

## Development Model

Use `once build <target>` for compile feedback and `once test <target>` for
tests. When one source file changes, Once reruns the target compile action and
the staging action. When sources and recorded dynamic inputs are unchanged, the
whole compile action can be reused from the cache.

Targets that use dynamic compile inputs learn those inputs during compilation.
The first build after adding a new dynamic marker may compile locally instead of
using a remote cache entry so the next cache key includes the discovered inputs.
After metadata exists, `@external_resource`, `Application.compile_env`, and
`__mix_recompile__?/0` participate in cache invalidation.

Phoenix projects should be represented as a graph instead of one broad target.
Macros or modules used across target boundaries should be separate
`elixir_library` targets. Third-party packages should also become Once-owned
dependency targets; pointing at precompiled build directories is only useful for
experiments because it puts compilation outside Once.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `app_name` | string | no | target name | Elixir application name; `-` and `.` are rewritten as `_` when omitted |
| `mix_env` | string | no | `prod` | Mix environment exported while compiling and testing, such as `prod`, `test`, or `dev` |
| `mix_config` | string | no | empty | Optional package-relative Mix project file included in the library action key when the project still needs one |
| `version` | string | no | `0.1.0` | Application version written to generated metadata |
| `description` | string | no | `""` | Application description written to generated metadata |
| `app_description` | string | no | `""` | Bazel-compatible alias used when `description` is omitted |
| `applications` | list&lt;string&gt; | no | `["kernel", "stdlib", "elixir"]` | Runtime applications written to generated metadata |
| `extra_apps` | list&lt;string&gt; | no | `[]` | Bazel-compatible runtime applications appended to `applications` |
| `included_applications` | list&lt;string&gt; | no | `[]` | Included applications written to generated metadata |
| `registered` | list&lt;string&gt; | no | `[]` | Registered process names written to generated metadata |
| `consolidate_protocols` | bool | no | `true` | Consolidate protocols after compilation and stage consolidated protocol bytecode into the application |
| `config` | list&lt;string&gt; | no | `["config/**/*.exs"]` | Config file globs included in the compile action key |
| `config_files` | list&lt;string&gt; | no | `[]` | Buck-compatible alias for additional config file globs |
| `data` | list&lt;string&gt; | no | `[]` | Data file globs available during compile |
| `priv` | list&lt;string&gt; | no | `[]` | Priv file globs copied into the application priv output |
| `resources` | list&lt;string&gt; | no | `[]` | Buck-compatible resource globs copied into the application priv output |
| `include` | list&lt;string&gt; | no | `["include/**/*.hrl"]` | Erlang header globs included in the compile action key |
| `docs` | list&lt;string&gt; | no | `[]` | Buck-compatible documentation file globs included in the compile action key |
| `os_env` | map&lt;string, string&gt; | no | `{}` | Buck-compatible environment variables exported before Elixir compile and test commands |
| `env_inherit` | list&lt;string&gt; | no | `[]` | Host environment variable names inherited before explicit `env` values |
| `env` | map&lt;string, string&gt; | no | `{}` | Environment variables exported before running Elixir compile and test commands |
| `compile_args` | list&lt;string&gt; | no | `[]` | Additional arguments appended to `elixirc` |
| `elixirc_opts` | list&lt;string&gt; | no | `[]` | Bazel-compatible alias for additional `elixirc` arguments |

Compatibility attributes declared for Buck and Bazel parity but not implemented
yet: `app_src`, `app_src_vsn`, `appup_src`, `erl_opts`, `ez_deps`,
`extra_includes`, `extra_properties`, `include_src`, `mod`, `shell_configs`,
and `shell_libs`. Non-empty values fail analysis.

## Dep Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `elixir_app` | Elixir applications available on the compile path |

## Providers

The target emits `elixir_app`.

## Provider Record

| Field | Type | Meaning |
| --- | --- | --- |
| `app_name` | string | Elixir application name |
| `mix_env` | string | Mix environment exported during compilation and tests |
| `ebin_dir` | string | Directory containing compiled application bytecode and the generated application file |
| `priv_dir` | string | Directory containing copied priv files |
| `compile_metadata` | string | File containing dynamic compile metadata used for future cache invalidation |
| `compile_warnings` | string | File containing persisted compiler warning diagnostics |
| `transitive_elixir_apps` | list&lt;record&gt; | This application plus dependency applications available to downstream Elixir targets |
| `transitive_sources` | list&lt;string&gt; | Source files from this target and Elixir dependencies |

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `bytecode` |

## Example

```toml
[[target]]
name = "greeting"
kind = "elixir_library"
srcs = ["lib/**/*.ex"]

[target.attrs]
app_name = "greeting"
```

Use [`elixir_test`](/reference/prelude/elixir_test) with a separate
`mix_env = "test"` library target when tests should reuse compiled bytecode.
