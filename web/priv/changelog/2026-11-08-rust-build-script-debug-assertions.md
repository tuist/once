---
title: Match Rust build-script debug assertions to their crates
date: 2026-11-08
---

Rust build scripts now use the same effective debug-assertions setting as the
crate they generate code for. This fixes optimized builds that consume files
generated only when debug assertions are disabled, including GPUI's Windows
shader bindings.

Explicit debug-assertions overrides take precedence over optimization defaults,
including when flags are repeated. Build scripts still compile for the host
and retain the default unwind panic strategy rather than inheriting arbitrary
target compiler flags.
