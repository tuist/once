---
title: Portable Dockerfile layer caches
date: 2026-09-26
---

Dockerfile image targets can export intermediate BuildKit layers as declared
outputs and reuse them through target dependencies. Once can store and restore
those layers with image archives, including Docker archives built with a
container or remote builder.

Registry cache imports and exports let independent builders share layers.
Select a remote BuildKit builder to run Dockerfile instructions away from the
local host while Once tracks the context inputs and exported artifacts.
