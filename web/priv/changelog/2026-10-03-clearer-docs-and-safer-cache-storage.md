---
title: Clearer documentation and safer cache storage
date: 2026-10-03
---

The documentation now offers a more direct path from your first cacheable
script to a typed build graph. The getting-started example demonstrates both
output restoration and input invalidation, and explains what you need to
declare for cache reuse to be correct.

A compact documentation home and previous/next links make the guides easier
to follow on desktop and mobile. The target-kind reference now includes
container composition, Dockerfile builds, Apple command-line tools, ShellSpec
tests, and script adapters. Navigation also includes previously missing
Elixir, Rust, and React Native reference pages.

Cache storage cleans up temporary files after failed writes and streamed-blob
installation. Host-tree hashing preserves literal Unix backslashes and
rejects non-UTF-8 paths rather than assigning ambiguous identities. Existing
metadata digest caches are refreshed automatically. Package-relative source
paths now reject absolute paths before joining them to a package directory.
