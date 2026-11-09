---
title: Preserve Cargo workspace checksums, features, and procedural-macro tests
date: 2026-11-09
---

Cargo workspace discovery now accepts registry checksums stored in legacy
lockfiles without requiring a lockfile format upgrade. Registry dependencies
still require a checksum from the lockfile.

Terminal color settings no longer introduce Cargo tree display markers into
resolved features or action keys. Procedural-macro unit tests can import the
compiler-provided `proc_macro` crate, matching native Cargo test compilation.

Cargo workspace seeds can explicitly configure generated tests with `test_env`,
`test_env_inherit`, and `test_cacheable`. Host-dependent suites can opt into
required tools and configuration with test-result caching disabled, while
compilation remains cacheable and the default test environment stays restricted.
