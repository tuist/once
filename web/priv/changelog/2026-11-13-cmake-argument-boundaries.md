---
title: CMake arguments keep their boundaries
date: 2026-11-13
---

`cmake_project` and `cmake_test` now pass each `configure_args`, `build_targets`,
and `build_args` item to CMake as exactly one argument. A configure argument
such as `-DFLAGS=-i100;-t1` sets a CMake list instead of being split at its
semicolons. Backslashes and unbalanced brackets are also preserved as written.

Native configure and build still bypass the cache.
