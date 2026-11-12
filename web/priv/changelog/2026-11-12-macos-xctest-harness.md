---
title: "Run macOS XCTest bundles with the Xcode harness"
date: 2026-11-12
---

Unhosted macOS XCTest bundles now use Xcode's `test-without-building` harness,
matching the runtime used by native Xcode tests without handing compilation
back to Xcode. Once still caches compilation, linking, and generated test-run
configurations, while test execution runs fresh.

Completed failed XCTest suites also retain their observed case inventory and
failure counts. Platform-conditional declarations no longer inflate a completed
failed run or cause the scheduler to replace its results with an unknown-runner
placeholder.
