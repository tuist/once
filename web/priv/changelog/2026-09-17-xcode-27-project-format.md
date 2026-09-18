---
title: Read the Xcode 27.2 project.xcproj JSON manifest
date: 2026-09-17
---

Once now recognizes the JSON-based `project.xcproj` manifest that Xcode 27.2
writes alongside, or in place of, the classic OpenStep `project.pbxproj`. A
converted project resolves through the same `xcode_workspace` target kind as
before, so target names, discovery, and downstream build actions stay
unchanged. Projects that ship both manifests prefer the JSON form; projects
that ship only `project.pbxproj` keep working exactly as they did.

The reader handles the shape choices that Xcode 27.2 uses in real projects,
including grouped and file-system synchronized folders with per-target
inclusion and exclusion lists, per-configuration setting suffixes such as
`ENABLE_TESTABILITY[config=Debug]`, per-configuration xcconfig links, string
and object build phases, remote and local Swift package references with
their product members, and short product-type spellings such as `framework`
or `bundle.unit-test`.

Two long-standing gaps that surfaced while validating the reader against
Alamofire, Kingfisher, RxSwift, MovieSwiftUI, and Ice Cubes are fixed in the
same release and benefit the classic manifest reader as well. Frameworks
whose hosted tests use `@testable import` now compile the dependency for
testing, so test bundles resolve the module they import. Frameworks that
themselves import XCTest, such as `RxTest`, are given the platform's
Developer framework and library search paths so they compile. Build
settings that nest one variable inside another, such as Alamofire's
`$(MACOSX_DEPLOYMENT_TARGET_XCODE_$(XCODE_VERSION_MAJOR))`, now resolve to a
concrete deployment target using the current Xcode's version.
