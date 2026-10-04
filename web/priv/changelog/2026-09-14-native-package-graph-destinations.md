---
title: Correct local and multi-platform Apple dependency graphs
date: 2026-09-14
---

Once now follows transitive local Swift package dependencies without requiring
a lockfile for an entirely local graph. Remote dependencies introduced by local
packages still use the root package's dependency resolution.

Xcode projects with both macOS and iPhone targets now get distinct dependency
graphs for each destination, including package targets and binary frameworks.
Manifest parsing is shared across destinations and local product discovery.

Package product dependencies retain every module in the product, even when a
module shares the product's name. Native application attributes and system
library dependencies also remain consistent with their graph contracts.

These improvements came from exercising Vapor and NetNewsWire. Small, offline
acceptance fixtures preserve their graph patterns and check dependency edges,
source membership, settings, selective builds, cache restoration, and changes
to transitive inputs without compiling the full applications.

Read about [Swift packages](/docs/guide/graph/swift-packages) and
[Xcode projects](/docs/guide/graph/apple/xcode).
