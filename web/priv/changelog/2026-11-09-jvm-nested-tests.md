---
title: "Discover nested JVM test suites"
date: 2026-11-09
---

Kotlin JVM and Android local tests now discover static nested test suites,
including failures that were previously omitted. Anonymous and non-static
inner helper classes remain excluded from independent test discovery.

Public inherited test methods also run on concrete suites. Overridden methods
are selected once, instead of executing a shadowed base implementation.

Kotlin JVM test results now use the standard test output location, so
`once query test-results` can read them after a run.
