---
title: Explicit modules for native Apple builds
date: 2026-09-14
---

Once can now use the Swift compiler's dependency scan to build imported Swift
and Clang modules as separate cacheable actions. Enable `explicit_modules`
on an Apple target or a native Xcode or Swift package workspace. Native Xcode
projects also honor `SWIFT_ENABLE_EXPLICIT_MODULES = YES`.

Shared module work reuses the action cache, and independent declared actions
can run concurrently within the build's memory budget. Module identity
includes compiler and development-kit contents. Existing builds keep their
implicit-module behavior unless explicit modules are selected.

An optional `dependency_check = "error"` rejects source imports of workspace
modules that are not declared dependencies. Errors include the target and a
suggested repair. Xcode's inferred import edges do not satisfy this check.

The module authoring contract now includes deferred action planning and host
directory digests. These are ecosystem-neutral primitives that other target
kinds can use. This release also fixes signing for standalone Apple executables.
Parent-relative dependency references now resolve correctly in nested Apple
packages.

See [Apple builds](/docs/guide/graph/apple) for configuration and current limits.
