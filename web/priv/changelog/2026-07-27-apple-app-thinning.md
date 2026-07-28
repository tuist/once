---
title: Device-specific Apple app size analysis
date: 2026-07-27
---

Once can now produce ad-hoc signed app archives for one Apple device model at a
time with `apple_thinned_package`. Keeping each model in its own target makes
the variants independently cacheable and ready for size-analysis services such
as Sentry without the noise of a universal build.

The target validates device builds, uses Xcode's thinning tool, re-signs
embedded bundles, and writes a deterministic archive plus a stable manifest.
Existing `apple_application` bundles now include the supported-platform
metadata that Xcode requires for thinning.
