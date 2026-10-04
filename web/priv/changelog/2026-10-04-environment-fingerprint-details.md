---
title: More precise environment evidence
date: 2026-10-04
---

Declared graph actions now record a separate fingerprint for each environment variable. When two builds have different action identities, you can compare their evidence to identify which variable changed without exposing its value.

The additional evidence does not change action cache keys or invalidate existing cached results.
