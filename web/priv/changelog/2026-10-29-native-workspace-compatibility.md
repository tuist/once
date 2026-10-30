---
title: "Native Xcode and Swift package compatibility fixes"
date: 2026-10-29
---

Pinned Swift package checkouts now reuse commits already present locally and
fall back to a full fetch when a Git server rejects shallow clients. The
`xcodebuild` wrapper accepts custom configuration names when they match the
Once seed, including `Development`.

Native package lowering now honors linked libraries, frameworks, unsafe linker
flags, and per-language warning policies. Manifest evaluation tracks a hash of
the inherited environment so changes to manifest switches invalidate cached
resolution without recording their values.

Discovery skips invalid local package templates with a warning. Resolver
failures make `once query targets` and its MCP equivalent fail rather than
present an incomplete graph as successful. Literal directory exclusions prune
descendants, and nested targets validate workspace runtime sources correctly.

Swift package build-tool plugins now run during native builds, including
source-built host tools, prebuild output discovery, generated resources, and
commands that declare no outputs. Plugin failures fail the build. Plugin
planning and execution run locally and bypass caching; native compilation
remains cacheable. Xcode workspace fixtures exercise application builds,
XCTest execution, environment changes, and regeneration after outputs are removed.
