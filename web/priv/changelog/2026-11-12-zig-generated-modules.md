---
title: Declare generated Zig modules
date: 2026-11-12
---

The new `zig_generated_library` target kind runs a declared host executable to
produce an importable Zig module. Code generators, input files, generated
outputs, and module dependencies are explicit graph edges and cache inputs.
Generators run directly, without shell wrappers or a nested `zig build`.

Set `emit_docs = false` on Zig binaries or compiled libraries to omit automatic
documentation generation. Existing targets continue to generate documentation
by default. This lets binary-only graphs match native builds without including
Zig's timestamp-bearing documentation source archives in their outputs.
