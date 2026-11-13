---
title: Nx plugin executors and cacheable test suites
date: 2026-11-12
---

Once can now invoke installed Nx plugin executors directly, while retaining
ownership of dependency scheduling, action caching, and output restoration.
Discovery retains the full project graph, including external package metadata
needed by compiler executors such as Nx 17's TypeScript compiler.
Resolved command tasks preserve the activated host toolchain path, so package
manager shims and SDK commands remain available.

Native Nx discovery now exposes application and library tests as cacheable
suite batches with normalized status, duration, and logs. Run them with
`once test`. Suite-level reporting does not yet provide individual case
listing, filtering, or sharding.

Workspace source hashing includes shared configuration and cross-project
sources, while excluding declared generated outputs. Nested output tokens
resolve against the default task configuration before caching.
