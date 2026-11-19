---
title: Nx task caches survive PATH changes
date: 2026-11-19
---

Nx tasks no longer inherit your shell's `PATH`, so activating another tool
version manager or opening a different terminal no longer invalidates every
cached task. A task's `PATH` now holds the project's `node_modules/.bin`
directories, the directories of Node.js and of the workspace's package
manager, and the system directories. A task that needs a tool installed
elsewhere can declare `{"env": "PATH"}` among its Nx `inputs` to receive your
shell's value as a cached input. Each Nx task runs once more after upgrading,
because its cache key changed.
