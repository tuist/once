---
title: Export native graph reproductions without source code
date: 2026-11-18
---

`once query graph-repro` writes a redacted copy of the workspace's native
projects to `.once/graph-repro`. Native manifests and lockfiles keep their
contents, and every other file is empty at the same path, so a graph that
fails to load or loads wrong reproduces without any source code. The export
records the Once version, the host platform, and the loaded graph or its
loading error. It also drafts an issue for the Once repository from those
facts, packs the export into `graph-repro.tar.gz` to attach, and prints a link
that opens the issue prefilled. Nothing is shared until you open it.

When a resolver fails in a way it does not explain itself, the error now
carries a repair that asks a coding
agent to offer this export and its drafted issue instead of changing the
project to work around Once. Agents get the export as the `once_export_graph_repro` Model Context
Protocol tool as well.

Resolver failures also report better. Their diagnostics and repairs now reach
`--format json` errors and Model Context Protocol tool errors instead of a
generic `operation_failed`, and human output lists repairs. Building a missing
target that a failed resolver owns, or testing a whole workspace whose
resolver failed, reports that failure instead of "no target matches" or an empty test
plan.

Module authors declare which files an export copies with the new `manifests`
argument to `native_project(...)`. It defaults to the markers.
