# `mix_workspace`

Native Mix project seed.

## Description

`mix_workspace` evaluates `mix.exs` for development, test, and production,
then emits ordinary `mix_dependencies`, `mix_project`, `elixir_test`, and
`mix_release` targets. The native manifest and lockfile stay authoritative.

For a leaf project, the seed's build capability selects the development
application. For an umbrella root, it selects the detected child
`mix_workspace` seeds. Nested projects and path dependencies preserve their
workspace-relative layout.

Projects with external dependencies must already have a current `mix.lock` and
materialized dependency sources. Resolution runs with isolated Mix and Hex
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
