---
title: Builds with a Swift toolchain outside Xcode
date: 2026-09-09
---

A Swift toolchain installed beside Xcode contributes the compiler, but the
linker and the software development kit still come from Xcode. Apple builds
now resolve those separately and pass the linker's location to every compile
and link, so a package that needs a newer compiler than the one Xcode ships
builds without falling back to whatever the surrounding shell happened to
expose.

Swift Testing follows the same rule. A macro plugin only matches the testing
library from its own toolchain, and the two ship on different schedules, so
test targets now compile and link against the copy the selected compiler
provides when it has one, and against the version Xcode publishes otherwise.
Test bundles that previously failed to expand `@Test` and `@Suite` build
again.

Swift package libraries are also force-loaded into what depends on them, the
way Swift Package Manager links every object file a target produced. A source
file whose only contribution is a protocol conformance is no longer dropped,
so conformances declared in an extension are found at runtime.
