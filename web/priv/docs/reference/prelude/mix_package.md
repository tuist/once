# `mix_package`

Synthetic vendored package generated from a locked Mix dependency graph.

## Description

`mix_package` is emitted by `mix_dependencies`; it is not intended for manual
declaration. Each target represents one Mix application and carries the exact
version, source identity, checksums, repository, and Git revision available in
`mix.lock`.

For a Mix-managed package, the target runs the compiler pipeline registered by
the vendored project. For a Rebar-managed package, it generates a sanitized
Rebar configuration, stages the package source into an isolated directory, and
invokes Rebar 3 directly. Dependency application outputs are declared inputs in
both modes. Hex runs offline, and the package writes bytecode, private files,
and include files into target-owned outputs.
Source-provided Rebar application metadata is staged before compilation and
validated afterward.

When the owning dependency set declares root configuration, Mix-managed
packages read it before compilation with the root graph environment and
target. The package then compiles with its own Mix environment, as reported by
the resolver.

A Make-only manager or custom dependency compile command fails before actions
are declared. This prevents Once from silently compiling a package with the
wrong semantics.

## Resolver-Owned Attributes

| Attribute | Type | Meaning |
| --- | --- | --- |
| `_mix_locked` | bool | Proves the target was generated with locked identity data |
| `_mix_package_name` | string | Hex registry package name |
| `_mix_source` | string | Stable Hex or Git source identity |
| `_mix_checksum` | string | Hex package checksum |
| `_mix_outer_checksum` | string | Hex registry checksum |
| `_mix_revision` | string | Locked Git revision |
| `_mix_repository` | string | Hex repository or Git repository identity |
| `_mix_managers` | list&lt;string&gt; | Build managers reported by Mix and the lockfile |
| `_mix_custom_compile` | bool | Whether the dependency overrides its compile command |
| `_mix_source_root` | string | Package-relative vendored source root |
| `_mix_manifest` | string | Root Mix manifest used during resolution |
| `_mix_lockfile` | string | Root authoritative lockfile |
| `_mix_root_config_entry` | string | Root configuration entry loaded before package compilation |
| `_mix_root_config_env` | string | Root graph environment used to read configuration |
| `_mix_root_config_target` | string | Root Mix target used to read configuration |

The target also receives the application metadata attributes shared with
`elixir_library`, including `app_name`, `version`, and `mix_env`.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `elixir_app` | Locked package dependencies reported by Mix |

## Providers

The target emits `elixir_app`.

## Provider Record

| Field | Type | Meaning |
| --- | --- | --- |
| `app_name` | string | Mix application name |
| `package_name` | string | Registry package name |
| `version` | string | Locked package version or Git revision |
| `source` | string | Stable locked source identity |
| `checksum` | string | Hex package checksum when present |
| `outer_checksum` | string | Hex registry checksum when present |
| `revision` | string | Git revision when present |
| `repository` | string | Hex repository or Git repository |
| `managers` | list&lt;string&gt; | Native build manager metadata |
| `ebin_dir` | string | Compiled application bytecode directory |
| `priv_dir` | string | Private application output directory |
| `include_dir` | string | Erlang include output directory |
| `compile_metadata` | string | Rebar application validation marker when Rebar is used |
| `compile_warnings` | string | Persisted compiler warning diagnostics |
| `transitive_elixir_apps` | list&lt;record&gt; | This package plus reachable package applications |
| `transitive_sources` | list&lt;string&gt; | Vendored sources for this package and its dependencies |

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `bytecode` |

## Sources

- [Mix compilation](https://hexdocs.pm/mix/Mix.Tasks.Compile.html) defines the
  registered compiler pipeline used by Mix projects.
- [Mix project configuration](https://hexdocs.pm/mix/Mix.Project.html)
  documents build paths, dependency paths, lockfiles, and project compilers.
- [Rebar 3](https://rebar3.org/) documents Erlang application compilation.
