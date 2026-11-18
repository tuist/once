---
title: "Immutable artifacts and explicit publication"
date: 2026-10-29
---

Struo can now export immutable, content-addressed build artifacts and publish
them independently of its action cache. Artifacts can contain arbitrary data
with optional versioned consumption contracts. Infrastructure owns distribution
and how people use the content.

Use `struo query artifacts`, `struo artifact export`, and `struo artifact publish`,
or their matching MCP tools. The built-in `artifact` target packages selected
files; Apple application targets export bundles and native metadata.
Local publication retains self-contained packages. The Tuist compatibility
adapter uploads supported application payloads to Tuist.

Publishing does not start compute. Tuist's current API does not support generic
artifact storage, atomic variant sets or idempotent republishing. Read
[Artifacts](/docs/guide/infrastructure/artifacts) for examples
and the current boundaries.
