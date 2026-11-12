---
title: Run JavaScript suites with duplicate test names
date: 2026-11-12
---

Jest and Vitest targets no longer fail an ordinary complete run just because
multiple assertions share a full test name in the same file. Once keeps the
complete native report and reports one whole-target result instead, preserving
native failures and successful action caching.

Inventories with ambiguous names stay whole targets on subsequent runs rather
than advertising exact case selection or sharding. Explicitly filtered runs
still reject ambiguity. Runner output now streams directly into the action log
so large snapshot failure reports cannot exhaust the adapter's output buffer.
