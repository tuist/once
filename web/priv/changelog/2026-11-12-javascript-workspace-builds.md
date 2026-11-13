---
title: Build JavaScript workspace packages
date: 2026-11-12
---

The new `javascript_build` target runs package scripts against a private copy
of a locked dependency installation and caches their declared products. Use it
for React Native library builds and Expo exports, independently of dependency
installation and native application compilation.

React Native build staging now preserves nested workspace dependency trees and
relative package links. Declare all workspace manifests, package-manager
configuration, and installation inputs to keep frozen installs reproducible.
