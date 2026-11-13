---
title: Reliable ShellSpec result reports
date: 2026-11-12
---

ShellSpec test reports now escape descriptions and paths correctly, so quotes,
backslashes, and control characters no longer prevent Once from saving results.
Repeated example descriptions receive distinct case IDs based on their spec
path and declaration line. These IDs remain stable across unchanged runs.
