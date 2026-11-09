---
title: Match native Apple resources and test inventories
date: 2026-11-09
---

Swift package `.process` resource directories now flatten ordinary files into
their resource bundle, matching SwiftPM. Explicit `.copy` directories keep their
hierarchy, and compiler-managed resources keep their bundle roots.

Swift package tests can invoke first-party package executables staged beside
their bundle. Packages using native alternate source layouts such as
`Source/<target>` retain Swift Package Manager's computed target paths.
Xcode products using `PROJECT_NAME` keep their native names,
including projects resolved through a workspace. Xcode seeds also accept
explicit native `build_settings` overrides, such as a deployment target shared
by framework and test targets.

Completed XCTest runs use native case completions and a matching successful
suite total instead of treating platform-conditional source declarations as
unexecuted tests. Missing or inconsistent native summaries still fail closed.

Xcode shared schemes now contribute test exclusions. An unhosted macOS XCTest
bundle honors those exclusions through native runner selection, and exclusions
that differ between shared schemes do not silently suppress tests.

Static libraries built on macOS now use deterministic archive metadata rather
than recording the current time in archive members. Framework links also use
the native linker's reproducible-output mode. Stable action keys and
bitwise-identical artifacts remain separate properties; independent cold-build
verification reports the artifacts it actually observes.
