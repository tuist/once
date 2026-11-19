---
title: Nx task caches survive HOME changes
date: 2026-11-19
---

Nx tasks no longer receive your `HOME`, so the same workspace checked out by
another user or on another machine can reuse their cached results. Tools fall
back to the account's home directory as they would without the variable. A
task that needs your `HOME` can declare `{"env": "HOME"}` among its Nx
`inputs`, which makes it part of the cache key. Each Nx task runs once more
after upgrading, because its cache key changed.
