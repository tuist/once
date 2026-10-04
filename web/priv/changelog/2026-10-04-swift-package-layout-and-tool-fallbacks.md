---
title: Resilient Swift package analysis and host tool fallbacks
date: 2026-10-04
---

Swift package analysis no longer fails when SwiftPM cannot fully describe a package, for example when a local binary artifact has not been produced yet or a declared test directory is missing from a downloaded dependency. Once keeps the manifest it already read and only asks SwiftPM for computed target paths when a target is not in its default location.

When a workspace's declared tools fall back to the host toolchain, Once no longer remembers those host paths for later runs, so the next build tries workspace resolution again.
