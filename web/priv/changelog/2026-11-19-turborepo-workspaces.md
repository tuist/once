---
title: Turborepo workspaces
date: 2026-11-19
---

Struo now loads [Turborepo](https://turborepo.com) workspaces without a
`struo.toml`. From a directory with `turbo.json`, `struo query targets`
lists one target per package and task, taken from turbo's own resolution
of `dependsOn`, `outputs`, `cache`, and environment variables.

`struo build` and `struo test` run each package script directly instead of
going through `turbo run`. Struo caches the outputs turbo declares and
restores them on a cache hit. Every task declares its package, the
workspace packages it depends on, installed dependencies, and the outputs
of the tasks before it, so `struo build --remote` can run uncached tasks
on any configured execution provider.

See the [Turborepo guide](/docs/guide/graph/turborepo) to get started.
