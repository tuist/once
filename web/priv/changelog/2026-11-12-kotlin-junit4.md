---
title: Native JUnit 4 execution for JVM tests
date: 2026-11-12
---

Kotlin JVM and Android local test targets can now set
`test_framework = "junit4"` to run through native JUnit 4 with declared runtime
dependencies. This preserves setup and teardown, rules, expected exceptions,
assumptions, and custom runners such as Robolectric while retaining cacheable
execution and normalized per-case Once results. Aggregate suites do not rerun
their child classes, and malformed annotated tests report initialization
failures. The default convention-based reflection runner is unchanged.
