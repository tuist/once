---
title: Select graph capabilities with experimental triggers
date: 2026-10-25
---

Root `struo.toml` triggers bind exact source refs to existing `build`, `test`,
and `run` capabilities with explicit target configurations. An existing CI
worker can execute those selections without maintaining another dependency
graph. Inspection, validation, atomic editing, explicit activation, planning,
and simulation are available through matching CLI and MCP interfaces.

Activation uses a separately chosen committed definition, so a candidate that
deletes or narrows its declaration does not silently remove its own selected
work. Simulations require the exact clean checkout and use a dedicated local
cache. CI still owns authenticated event delivery, worker isolation, resource
ceilings, and required-check publication. This is not a native controller or
an immutable replay facility.

Read [Event-Driven Automation](/docs/guide/infrastructure/triggers) for the
service integration model and current support status. A
[Buildkite example adapter](/docs/guide/infrastructure/buildkite) generates
worker jobs from approved triggers through dynamic JSON pipelines. Tuist
orchestration support is coming; no native service adapter is released yet.
