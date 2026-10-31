---
title: Reuse Swift package layout discovery
date: 2026-10-31
---

Warm Xcode graph loads now reuse successful Swift package layout descriptions
as well as manifest dumps. Packages using layouts such as `Source/` no longer
restart SwiftPM on every unchanged graph query.

Manifest and source changes, added or removed files, compiler changes, and
environment changes invalidate the saved answer. Layout discovery stays
offline, and failed queries are retried rather than cached.
