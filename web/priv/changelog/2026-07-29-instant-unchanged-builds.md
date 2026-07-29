---
title: Near-instant unchanged builds
date: 2026-07-29
---

Once now returns almost immediately when nothing relevant has changed. A build
that already matches the cache no longer revalidates the whole graph on every
invocation. Instead, Once records what changed since the last successful build
and reuses the previous result when the sources, declared outputs, environment,
and tools it observed are all unchanged.

Input checking is also cheaper. Once validates unchanged files by their
metadata rather than rereading their contents, so a repeat build no longer pays
to rehash the entire dependency tree. When a cached result is reused, only the
outputs that actually differ are restored. Together these reduce both the time
and the memory an unchanged or lightly changed build needs.

Correctness is preserved conservatively: any change to a watched source or
output, a relevant environment variable, or an observed tool on disk falls back
to a full validation before the fast path is trusted again.
