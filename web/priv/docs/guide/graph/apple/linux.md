# Build iOS Applications on Linux

Once can cross-compile native Apple targets on Linux using Swift and the Darwin
Swift SDK installed by [xtool](https://xtool.sh). Once still owns compilation,
linking, resource packaging, caching, and action scheduling. It does not run
`xtool dev build` behind the scenes.

The [omarchy-apple-dev](https://github.com/joshuaswarren/omarchy-apple-dev)
project provides an installer and Linux replacements for Apple resource tools.
Its installer supports Omarchy's Arch-based environment on ARM64 and x86_64.
Once's integration uses the installed tools and SDK rather than depending on
that distribution's package manager.

## Install the Toolchain

Follow omarchy-apple-dev's installation guide. You need:

- A Linux Swift toolchain matching the Swift version in the selected Xcode.
- An xtool Darwin SDK built from your own Apple-provided `Xcode.xip` download.
- `rcodesign` for ad-hoc bundle signing.
- The Linux `actool`, `momc`, and `ibtool` compatibility tools when your targets
  use asset catalogs, Core Data models, storyboards, or xibs. Their supported
  resource formats are determined by omarchy-apple-dev, not by Once.
- `llvm-lipo` on `PATH` if you build multi-architecture libraries. The xtool
  toolset supplies its linker and archiver but does not always include lipo.

Apple's SDK is not downloaded or redistributed by Once. Obtain Xcode from
Apple with an Apple ID and install the SDK with xtool:

```sh
xtool sdk install /path/to/Xcode.xip
swift sdk list
```

The SDK list must include `darwin`. Run the installer with your downloaded
Xcode archive to also install the compatibility tools:

```sh
XCODE_XIP=/path/to/Xcode.xip ./install-toolchain.sh
```

Use the Swift toolchain's own Clang when installing the SDK. Host Clang headers
from a different compiler version can make SwiftUI fail to compile. Once
resolves both Swift and Clang from the active Swift toolchain for its actions.
If your toolchain requires a compatibility `LD_LIBRARY_PATH`, Once passes it
through to compiler and resource actions and includes it in their cache identity.

## Build a Starter

Materialize the Linux starter in an empty directory:

```sh
once edit materialize-example apple_application apple-application-linux
once build Hello
```

The starter links a Swift library into an iOS application. Both targets use:

```toml
[target.attrs]
platform = "ios"
sdk_variant = "device"
minimum_os = "17.0"
```

Set `sdk_variant = "device"` on dependencies as well as the application.
The default remains `"simulator"`; Once does not silently change it on Linux.
An iOS device target defaults to ARM64 even on an x86_64 Linux host. Library
`archs` overrides still apply.

Once discovers SDK and Swift resource paths through
`swift sdk configure darwin <target-triple> --show-configuration`. It uses the
SDK's Mach-O linker and archiver, rather than the host's ELF tools. SDK contents,
tool binaries, and the compiler environment contribute to Linux action cache
identity. Changes to an SDK therefore invalidate affected actions.

To pin an installed SDK, set the existing `xcode_developer_dir` attribute to the
SDK bundle's `Developer` directory on every relevant target:

```toml
xcode_developer_dir = "/home/me/.swiftpm/swift-sdks/darwin.artifactbundle/Developer"
```

This selects SDK metadata and tools, not macOS executables extracted from Xcode.
After switching Swift installations, re-register the matching SDK using
omarchy-apple-dev's `install-toolchain.sh --repair`.

## Deploying and Shipping

Once produces an **ad-hoc signed** application bundle. It does not provision an
Apple identity, install a provisioning profile, or upload a build. The bundle
must be provisioned and re-signed before a physical iPhone will accept it.
Use xtool or omarchy-apple-dev's documented signing and USB deployment workflow
for those steps. Its `ship.sh` workflow separately handles distribution signing,
validation, and App Store Connect uploads; a Once build is not automatically
an App Store-ready archive.

## Boundaries

- Native `apple_library`, `apple_framework`, `apple_application`, and
  `apple_executable` builds use the Linux toolchain. Available destinations
  are limited to those present in your Darwin SDK.
- Apple binaries cannot execute on Linux. Simulator runs, XCTest and Swift
  Testing bundles, Xcode app thinning, and Intent definition code generation
  still require macOS.
- `swift_macro` builds still require macOS. Prebuilt compiler plugins supplied
  through `binary_swift_plugins` must be Linux executables. SDK-provided macros
  use the SDK's OpenAppleMacros server when installed.
- Native Xcode and Swift package workspace resolution is not enabled by this
  integration. Declare native Apple targets in `once.toml` for Linux builds.
- Mac Catalyst and platforms absent from the SDK are not supported by this
  Linux integration.
