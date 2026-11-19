---
title: Mise-managed action caches survive HOME changes
date: 2026-11-19
---

Graph actions that run through the workspace's mise toolchain no longer key
their cache on your `HOME`, Struo's data and cache directories, or the
workspace's absolute path. Machines and users with different home or cache
directories can now reuse each other's results. Each such action runs once
more after upgrading, because its cache key changed.
