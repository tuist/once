---
title: More reliable local memory budgets and change tracking
date: 2026-11-05
---

On Linux, automatic memory budgeting now uses the current process's readable
cgroup limits, including visible ancestor limits, instead of relying only on
host memory. Memory detection also preserves inherited open-file limits.
Explicit memory overrides keep their existing behavior. The budget remains an
invocation-local scheduling estimate, not a hard operating-system ceiling or
a host-wide reservation pool. See [memory limits](/docs/guide/local-execution/memory-limits).

The change tracker now starts without depending on shell-specific descriptor
syntax and closes inherited descriptors before starting its runtime. Read-only
file access no longer invalidates otherwise unchanged inputs; writes and
uncertain watcher events still invalidate conservatively.
