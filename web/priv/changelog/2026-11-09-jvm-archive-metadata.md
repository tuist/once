---
title: Deterministic JVM archive metadata
date: 2026-11-09
---

Android JAR and AAR packaging and APK additions now use fixed entry timestamps,
preventing filesystem modification times from changing cached artifacts.

Kotlin JVM libraries can declare a JAR `manifest` input for publication metadata
required by downstream compiler plugins. Once compiles a declared classes tree
and packages it with the supplied manifest using JDK 17 or newer `jar`. The
manifest contents and archive tool identity are included in caching. Libraries
without a manifest keep their existing compiler-produced archive behavior.
