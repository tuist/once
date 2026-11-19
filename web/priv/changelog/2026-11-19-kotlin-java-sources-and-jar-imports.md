---
title: Mixed Kotlin and Java sources, and prebuilt jar imports
date: 2026-11-19
---

Kotlin JVM libraries, binaries, and tests now compile `.java` files listed in
`srcs` together with the Kotlin sources, so either language can reference the
other, and Java test classes run alongside Kotlin ones. The new `java_import`
target kind puts prebuilt jars, such as Maven artifacts, on Kotlin and Android
classpaths. JUnit 4 suites ignored at the class level now report one skipped
case per test method, matching Gradle's counts.
