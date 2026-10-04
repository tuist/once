---
title: Stable managed tool resolution
date: 2026-10-04
---

Once no longer treats developer-installed mise shims as concrete executables when resolving a workspace's declared tools. If workspace resolution is unavailable, it finds the underlying host tool instead of routing the action through a second mise installation.

This keeps parallel builds from alternating between shim and host-tool search paths. Cached tool-path records that point at mise shims are rejected; valid records remain reusable.
