---
title: Declare Bazel environments and test selection
date: 2026-11-12
---

Bazel workspace seeds now accept an `env` map in `once.toml`. The declared
environment follows discovery, analysis, and generated build and test targets.
Empty values override inherited host variables, so an unrelated SDK installation
does not have to affect repository analysis.

Once also finds `bazelisk` when the default `bazel` executable is missing.
Custom executable selections are preserved. Declaring an environment alone does
not enable spawn caching.

The new `test_query` attribute selects default test roots without removing build
targets. This lets a workspace declare a test suite matching its native tag
filters while keeping benchmarks and other excluded tests available to build.
