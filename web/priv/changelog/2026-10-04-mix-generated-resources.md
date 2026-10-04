---
title: Preserve generated Mix dependency resources
date: 2026-10-04
---

Once now preserves compiler-generated `priv` and `include` files when building Mix dependencies, including native libraries produced by dependency compilers.

Static package resources are combined with generated resources inside the compilation action instead of replacing them afterward. Both are captured in the cached application output, so cached builds retain the files needed to start dependent applications.
