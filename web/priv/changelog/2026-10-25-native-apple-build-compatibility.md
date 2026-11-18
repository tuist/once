---
title: "Improve native Apple build and test compatibility"
date: 2026-10-25
---

Struo handles more existing Xcode projects and Swift packages without changing
their sources or delegating compilation to Xcode.

Swift packages use single-worker batch compilation by default, avoiding repeated
frontend startup and parsing for each source file. Whole-module optimization
and explicit package compiler flag overrides remain supported.

Swift package dependency resources are installed in the macOS test bundle's
resources directory, and generated `Bundle.module` accessors find them there.
Package C and C++ targets honor conditional header search paths.

Frameworks that import XCTest receive its Swift overlay search path.
Objective-C-only test bundles link through Clang, avoiding unnecessary Swift
runtime compatibility libraries. Swift compilation and linking no longer load
those libraries twice when a target requests all archive members.

Embedded framework signing preserves versioned macOS layouts instead of creating
a signature directory at the framework root.
Resource bundles and help books also retain their complete directory trees.
Synchronized groups preserve explicit resource folders and include web resources
and scripting dictionaries. AppleScript sources compile into `.scpt` resources.

Explicit macOS `CODE_SIGNING_ALLOWED=NO` configurations omit entitlement-bearing
signing while retaining an ad-hoc bundle seal for local integrity checks.
Capabilities that require authorized signing remain disabled in this mode.

Xcode imports accept build phase scripts stored as arrays of lines, normalize
relative script paths, and resolve package tool dependencies declared through
target dependencies. Linked framework dependency matching also handles spaces
in target names. Scripts receive transitive framework products and selected
developer and toolchain paths.

Swift builds honor native regular-expression and concurrency defaults. String
catalogs compile into localized runtime resources and generate Swift symbols
when the target enables them. Application-hosted macOS tests use Xcode's test
runner without asking Xcode to build the products.

Test execution uses each scheme's default plan instead of combining exclusions
and arguments from its alternative plans.

Test discovery preserves the enclosing suite across nested helper types,
comments, strings, and local declarations, preventing duplicate test identities
from corrupting the reported results. Source discovery does not imply execution:
tests without an observed result remain unknown.
