---
title: Run native CTest suites through typed graph targets
date: 2026-11-09
---

The new `cmake_test` target configures and builds an existing CMake project,
then runs CTest with normalized suite-level results, a retained log, and native
JUnit. Failing tests and empty suites fail Once. Native case filtering and
sharding are explicitly unsupported.

Native configure/build actions for both `cmake_test` and `cmake_project` now
bypass caching. CMake can select external compilers, toolchains, and SDKs
that Once cannot safely fingerprint, so every invocation cleans and rebuilds
rather than risking stale executables. Product staging remains
content-addressed. Relocated sandbox and remote execution of `cmake_test`
fail explicitly because native build trees contain absolute paths.

Tests always run uncached. A positive `timeout_ms` enforces a suite deadline,
excluding configure and build, and retains normalized failure on timeout.
Debug information is preserved.
