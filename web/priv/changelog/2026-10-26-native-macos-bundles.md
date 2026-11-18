---
title: "Build runnable native macOS application bundles"
date: 2026-10-26
---

macOS applications now use the native `Contents` layout. Executables live in
`Contents/MacOS`, resources and compiled asset catalogs in `Contents/Resources`,
and embedded frameworks in `Contents/Frameworks`. The framework runpath is set
automatically, and generated property lists use macOS deployment metadata.
iOS bundle paths are unchanged. Struo rebuilds legacy flat macOS products
without leaving obsolete files beside the new bundle contents.

Native Xcode script phases receive the matching executable, property-list, and
resource-directory settings. Scripts that previously assumed a flat macOS
bundle must update their paths.

The `apple-application-macos` starter is discoverable through the CLI and MCP.
It includes a SwiftUI application and a bundled resource, ready for `struo run`.
The Apple guide also explains how to sign and notarize a private release copy
without exposing release credentials or changing cached build products.
Builds remain ad-hoc signed; Developer ID signing and provisioning are separate
release responsibilities.
