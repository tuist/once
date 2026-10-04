---
title: Dockerfile instructions as individual actions
date: 2026-09-27
---

Once now translates existing Dockerfiles into individual image snapshot
actions. Earlier instructions can reuse cached snapshots when later sources
change, and cross-stage copies declare their dependencies explicitly.
Container and remote BuildKit builders execute each instruction.
Once also discovers a `Dockerfile` as an `image` target without `once.toml`.
When Docker's built-in builder is the default, Once provisions a reusable
container builder automatically.

Dockerfile targets also expose advisory lint findings for inputs that may
change outside the declared action contract. Findings include source locations
and suggested repairs for agent review, without changing caching policy.

Dockerfiles that use heredocs, `ONBUILD`, or a `# syntax=` frontend build as a
single whole-file BuildKit action automatically, and `once lint` reports why.
Cacheable images now require base images pinned by digest, so a moving tag
cannot serve a stale cached image.
