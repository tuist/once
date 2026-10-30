---
title: Reliable local package discovery and shared source reuse
date: 2026-10-29
---

Xcode package discovery now follows explicit local package dependency paths before
searching the workspace. Duplicate products or conflicting package identities
report both candidate paths and explain how to select the intended dependency,
instead of silently choosing a stale copy.

Remote Swift packages reuse a shared Git repository across worktrees while keeping
their source checkout workspace-local. Pinned commits remain available to other
worktrees, and failed acquisitions do not publish partial checkouts. Servers that
reject shallow fetches still work through an ordinary-fetch fallback.

Warm graph loads reuse successful manifest dumps for referenced packages.
Manifest edits, source changes, added or removed package files, tool changes and
environment changes invalidate the saved metadata.
