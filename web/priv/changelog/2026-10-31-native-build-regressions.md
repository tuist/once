---
title: Native script caching and app extension bundles
date: 2026-10-31
---

Native Xcode script phases inherit the caller's environment and receive dSYM
location settings. Unresolved build-setting expressions in either `$(NAME)` or
`${NAME}` form no longer become literal filesystem inputs. Application compiler
actions track their declared inputs independently, so an unrelated directory
output from an always-run script does not disable compile-cache reuse.

Swift application module actions assign explicit per-source intermediate paths
inside build outputs. App extensions build as `.appex` bundles and are embedded
in their host's `PlugIns` directory before signing.

Local Swift package paths that refer to the same directory are accepted even
when their spelling differs on a case-insensitive filesystem. Repository
acquisition failures identify the checkout and repository without exposing URL
credentials, query strings, or Git output.
