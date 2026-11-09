---
title: Native pytest collection and JavaScript suite units
date: 2026-11-09
---

Pytest targets now preserve native collection rules such as `collect_ignore`
while restricting collection to declared sources. Exact case selection happens
after collection, so it cannot bring ignored modules back into a run.
Package-local configuration is discovered from the source paths, and project
ignore hooks cannot admit undeclared test files.

Vitest and Jest targets can opt into `unit = "suite"` for suites with duplicate
full test names. Once records one suite result and preserves every assertion
in the native report. This mode does not support case listing, filtering, or
automatic sharding. The default case mode still rejects ambiguous names.
