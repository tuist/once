# `react_native_android_application`

React Native application for Android.

## Description

Stages a checked-in Android project and invokes its pinned
[Gradle](https://gradle.org/) wrapper through Java. This preserves React
Native's New Architecture integration for Codegen, autolinking, CMake, Prefab,
and Hermes. The run capability installs the application through the
[Android Debug Bridge](https://developer.android.com/tools/adb), forwards
Metro's port, and launches the application.

## Required attributes

`application_id` identifies the Android application. Common optional
attributes are `native_root`, `module`, `configuration`, `architectures`,
`launch_activity`, `metro_port`, `node`, `java`, `adb`, `adb_serial`,
`android_sdk`, `android_ndk`, `allow_network`, `gradle_args`, `apk_path`, and
`exclude_srcs`.

Set `adb_serial` when more than one device is connected. The run capability
waits for the selected device, installs the application, forwards Metro's port,
and then launches the configured activity or the resolved launcher activity.

The default Gradle output is
`android/app/build/outputs/apk/<configuration>/app-<configuration>.apk`.
Set `apk_path` for product flavors, a differently named module, or an unsigned
release package. Generated Gradle build directories are excluded while staging;
`exclude_srcs` adds project-specific exclusions.

The `deps` edge accepts a React Native dependency set plus optional code
generation, autolinking, and release-bundle providers. Non-debug variants stage
the JavaScript sources from the matching Android bundle provider before the
checked-in Gradle bundling task runs.

## Capabilities

| Capability | Output groups | Requires |
| --- | --- | --- |
| `build` | `default`, `apk`, `logs` |  |
| `run` | `default` | `apk` |

Application installation, port forwarding, and launch are non-cacheable
runtime effects.
