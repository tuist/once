# `apple_executable`

Builds an Apple command-line executable without an application bundle. Use it
for a Swift tool, or for a native Xcode target whose product type is a tool.
The binary is ad-hoc codesigned in place.

Linux builds use Swift, xtool's Darwin SDK, and `rcodesign`. The existing
`xcode_developer_dir` attribute selects the SDK bundle's `Developer` directory
on Linux, defaulting to the registered `darwin` Swift SDK. See
[Build iOS Applications on Linux](/guide/graph/apple/linux) for setup.
The `run` capability requires macOS; Apple binaries do not execute on Linux.

## Sources and dependencies

Set the required `platform` and declare Swift sources with `srcs`. Deployment
version, SDK variant, developer directory, module name, compiler options, and
linker options follow the [Apple target model](/guide/graph/apple).

The `deps` role accepts `apple_linkable`, `apple_framework`,
`apple_swift_plugin`, and `native_linkable` providers. Runtime frameworks and
resource bundles are staged beside the executable. Mixed-language tool
targets lowered from Xcode are not currently supported; put their Objective-C,
C, or C++ code in an `apple_library` dependency.

## Capabilities

The target exposes `build` and `run` with the `default` output group. Running
requires the built executable and its staged runtime dependencies.

```sh
once query schema apple_executable
once query example apple_executable apple-executable-minimal
```

See [Xcode Projects](/guide/graph/apple/xcode) when adopting an existing project.
