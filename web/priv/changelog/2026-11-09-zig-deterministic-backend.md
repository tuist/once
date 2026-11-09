---
title: Use LLVM for cacheable Zig compilation
date: 2026-11-09
---

Zig targets now select the LLVM backend by default. This avoids differing
executable bytes observed with Zig 0.16's self-hosted backend when compiling
identical inputs with fresh compiler caches.

The selected Zig compiler must include LLVM support for the requested target.
Explicit `-fno-llvm` options remain supported, but compilation and
documentation actions bypass the cache rather than treating a stable action
key as evidence of byte reproducibility. Compiler failures are reported without
silently switching backends.

Unstripped compilation also bypasses caching because LLVM debug and symbol
metadata can differ across identical builds. Debug information remains enabled
by default. Explicitly enabling stripping permits caching with LLVM; Once does
not remove debugging information automatically.
