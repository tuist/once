# `react_native_apple_application`

React Native application for Apple platforms.

## Description

Stages a checked-in native project, installs its locked Ruby dependencies,
runs [CocoaPods](https://cocoapods.org/) in deployment mode, and builds the
workspace with [Xcode](https://developer.apple.com/xcode/). The run capability
installs and launches the product on a booted
[iOS Simulator](https://developer.apple.com/documentation/xcode/running-your-app-in-simulator-or-on-a-device).

The target supports the React Native
[New Architecture](https://reactnative.dev/architecture/landing-page), native
module autolinking, Codegen, and Hermes.

## Required attributes

`bundle_id` identifies the application. Common optional attributes are
`product_name`, `native_root`, `workspace`, `scheme`, `configuration`, `sdk`,
`simulator`, `exclude_srcs`, `node`, `bundle`, `xcodebuild`, and
`allow_network`.

The `sdk` value may be `iphonesimulator` or `iphoneos`. Simulator builds disable
code signing and can be installed through `once run`. Device builds use the
signing settings in the checked-in Xcode project and are build-only in Once.
Generated CocoaPods and native build directories are excluded while staging;
`exclude_srcs` adds project-specific exclusions.

The `deps` edge accepts a React Native dependency set plus optional code
generation, autolinking, and release-bundle providers.

## Capabilities

| Capability | Output groups | Requires |
| --- | --- | --- |
| `build` | `default`, `app`, `logs` |  |
| `run` | `default` | `app` |

Installation and launch are deliberately non-cacheable runtime effects.
