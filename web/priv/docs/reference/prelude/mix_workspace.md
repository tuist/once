# `mix_workspace`

Native Mix project seed.

## Description

`mix_workspace` evaluates `mix.exs` for development, test, and production,
then emits ordinary `mix_dependencies`, `mix_project`, `elixir_test`, and
`mix_release` targets. The native manifest and lockfile stay authoritative.

For a leaf project, the seed's build capability follows `mix_env`, then
`MIX_ENV`, defaulting to the development application. For an umbrella root, it selects the detected child
`mix_workspace` seeds. Nested projects and path dependencies preserve their
workspace-relative layout. From an unconfigured project directory,
`once build` and `once test` automatically select the current Mix project.
The nearest Git repository root contains sibling path dependencies without
requiring an extra manifest. Unrelated native integrations are not resolved by default. The same boundary
and family scope apply to run, lint, and `--all`; an explicitly selected native
seed in the selected package can choose another family. Repository-root commands
without an explicit native target keep ordinary discovery, and containing
explicit manifests take priority.

Native compilation includes non-hidden project files outside generated build
and dependency directories as conservative data inputs. Compile-time resources
and custom compiler sources therefore invalidate cached application output.
Compilation identities also include the source location, so bytecode containing
directory-sensitive resources is not reused across different checkout paths.

Native test targets use Mix's configured test paths and aliases. They run on
every invocation and inherit the host runtime environment so database setup
and host tools behave like an ordinary `mix test`. Application and dependency
compilers run through Once's build actions, not again inside the test alias.

Projects with external dependencies must already have a current `mix.lock` and
materialized dependency sources. A project with only local path dependencies
can omit its lockfile. Resolution runs with isolated Mix and Hex
homes and Hex offline mode.

Materialize the exact locked sources before previewing or loading the graph:

```sh
mix deps.get --check-locked
```

Use `mix deps.get` without `--check-locked` only when intentionally creating
or updating the lockfile.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `mix_env` | string | no | empty | Default build environment; otherwise follows `MIX_ENV`, then `dev` |
| `manifest` | string | no | `mix.exs` | Package-relative Mix project manifest |
| `lockfile` | string | no | `mix.lock` | Package-relative authoritative lockfile |
| `resolver_inputs` | list&lt;string&gt; | no | `srcs` | Text inputs available while deriving the graph |

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `mix_project`, `mix_workspace` | Default development application or nested workspace selected by the resolver |

## Providers

The target emits `mix_workspace`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | none |

## Direct Use

Inspect the automatically derived graph:

```sh
once query workspace
once query targets
```

No `once.toml` is required. To configure the resolver explicitly, author a
target equivalent to:

```toml
[[target]]
name = "mix"
kind = "mix_workspace"
srcs = ["mix.exs"]

[target.attrs]
resolver_inputs = ["mix.exs", "mix.lock", ".formatter.exs", "config/**/*.exs"]
```

## Sources

- [Mix project configuration](https://hexdocs.pm/mix/Mix.Project.html)
  defines the evaluated project metadata.
- [Mix tasks](https://hexdocs.pm/mix/Mix.Task.html) define project task
  behavior.
