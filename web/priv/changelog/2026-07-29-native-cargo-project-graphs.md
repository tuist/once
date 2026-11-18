---
title: Native Cargo project graphs
date: 2026-07-29
---

Struo can now read an existing `Cargo.toml` and turn it into a typed, cacheable
build graph. Rust developers can build, run, and test a workspace without first
translating it into `struo.toml`.

Native discovery understands Cargo workspaces and the locked dependency closure
from `Cargo.lock`, including registry, Git, and path packages. Each workspace
member and locked dependency compiles as its own action, so unchanged work can
be restored independently from a local or remote cache. Procedural macros and
build scripts are modeled as their own targets so they rebuild only when their
own inputs change.

Struo keeps `Cargo.toml` and `Cargo.lock` authoritative and uses the Rust
toolchain selected by the project's configuration. Test targets report their
results through the same normalized shape as the other ecosystems, so scheduled
runs, change-scoped selection, and summaries work the same way.

Start by running `struo query targets`, then use `struo build`, `struo run`, and
`struo test` with the target identifiers Struo reports.
