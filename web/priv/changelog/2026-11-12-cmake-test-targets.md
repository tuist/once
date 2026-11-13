---
title: Native CMake test targets
date: 2026-11-12
---

`cmake_test` now supports `test_targets` for projects that expose custom native
test targets rather than CTest entries. Once invokes those targets through
`cmake --build`, retains suite-level results and logs, and reports native
failures and timeouts without inventing a JUnit report.

The default still uses CTest and fails when no tests are found. Native builds
and test execution remain uncached until external toolchain selections can be
fingerprinted safely. Content-addressed product staging is unchanged.
