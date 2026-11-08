---
title: Explicit sibling source inputs for Cargo build scripts
date: 2026-11-08
---

Cargo build scripts can now declare individual source files from other locked
packages through `build_script_sources` on `cargo_workspace` and
`cargo_dependencies`. Once snapshots these files into a per-target sibling
layout alongside the consumer's own sources. Versioned vendor directories and
Cargo's local source cache remain unchanged.

The declared files participate in action caching and sandbox inputs. Warm builds
also revalidate observed host files and source trees, including changes outside
the workspace. See the [Rust guide](/docs/guide/graph/rust).
