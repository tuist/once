# `react_native_module`

One locked React Native native module.

## Description

`react_native_dependencies` generates this target from checked-in autolinking
metadata. It records the package version plus CocoaPods, Gradle, Fabric, CMake,
and native-library metadata needed to inspect the native dependency graph.
Packages should not declare this target directly.

## Providers and capabilities

The target emits `react_native_module`. Its `build` capability has no output
groups because it represents locked metadata; the owning dependency target
materializes package contents.

## Attributes

The resolver owns `module_name`, `version`, `source_root`, `ios_podspec`,
`android_source_dir`, `android_library_name`, `android_cmake_lists`,
`android_component_descriptors`, and `pure_cxx`.

