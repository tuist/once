---
title: Android major and minor SDK platform discovery
date: 2026-11-12
---

Android targets now recognize installed SDK platforms such as API 37.0, in
addition to integer API versions. Automatic selection compares release versions
numerically and skips preview and canary platforms. An explicit
`compile_sdk = 37` also finds the 37.0 platform when the integer platform is not
installed.

The selected platform version remains part of the action cache identity, while
resource linking uses the integer API level for the default target SDK.
