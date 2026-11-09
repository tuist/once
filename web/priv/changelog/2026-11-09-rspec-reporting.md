---
title: RSpec reports without application CLI collisions
date: 2026-11-09
---

RSpec targets now select their JSON formatter by its fully qualified class
name. Tests that inspect command-line arguments no longer mistake Once's
reporting option for an application's `--format json` option. Native reports,
exact example selection, and failing-test propagation are preserved.
