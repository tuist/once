---
title: Cached fallback discovery and parallel package acquisition
date: 2026-10-30
---

Xcode's fallback search for local Swift package products now reuses successful
manifest dumps across graph loads. Manifest and source edits, added or removed
files, tool changes and environment changes invalidate the saved answer. Invalid
templates still produce a warning and are retried rather than saved as failures.

Xcode and native Swift package workspaces acquire independent pinned Git
repositories concurrently before reading their manifests. Acquisition shares
Once's CPU and memory admission budget with build execution, including
`--cpu-limit` and `--memory-limit`. Shared repository locking, offline reuse,
pinned revisions and ordinary-fetch fallback remain unchanged.
