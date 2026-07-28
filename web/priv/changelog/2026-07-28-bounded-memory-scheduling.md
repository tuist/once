---
title: Bounded memory scheduling for local actions
date: 2026-07-28
---

Once now schedules concurrent local actions against a memory budget. The
default is two thirds of the memory visible to the host or container, while the
new global `--memory-limit` option lets constrained machines choose a smaller
budget.

Builds, lint targets, run targets, literal commands, scripts, and test workers
share the same limit. Actions without a declared estimate receive a conservative
default, and oversized actions run alone instead of waiting forever.

Large command streams, file outputs, directory outputs, and cache uploads or
downloads now move through bounded buffers and temporary files instead of
requiring complete artifacts in memory.
