---
title: "Respect ignored JVM tests"
date: 2026-11-09
---

Kotlin JVM and Android local test targets now honor method-level and
class-level `Ignore` annotations. Ignored tests are reported as skipped,
without running their bodies or initializing ignored test suites.
