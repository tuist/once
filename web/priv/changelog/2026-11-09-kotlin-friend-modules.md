---
title: "Compile against multiple Kotlin friend modules"
date: 2026-11-09
---

Kotlin JVM targets can now access internal declarations from multiple
`associates`, including a friend library that has transitive dependencies.
Friend module paths use the Kotlin compiler's list format independently
of the host platform's classpath separator.
