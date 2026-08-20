---
title: Native Mix project graphs
date: 2026-07-28
---

Once can now read an existing `mix.exs` and turn it into a typed, cacheable
build graph. Elixir developers can build an application, run formatting and
dependency checks, execute tests and Mix tasks, and assemble releases without
first translating the project into `once.toml`.

Native discovery understands locked Hex, Git, path, Mix, and Rebar 3
dependencies. Each package compiles as its own action, so unchanged dependency
work can be restored independently from a local or remote cache. The graph also
keeps development, test, and production environments separate.

Once uses the Elixir and Erlang versions selected by the project's toolchain
configuration while keeping `mix.exs` and `mix.lock` authoritative. Umbrella
applications and nested Mix projects receive package-qualified targets, and
structured diagnostics explain unsupported dependency build managers or
missing materialized sources.

Start by running `once native show mix`, inspect the derived targets
with `once query targets`, then use `once build`, `once run`, and `once test`
with the target identifiers Once reports.
