---
title: Build iOS applications on Linux
date: 2026-10-07
---

Once can cross-compile native Apple libraries, frameworks, applications, and
executables on Linux using Swift and xtool's Darwin SDK. Compilation, linking,
resource packaging, and ad-hoc signing remain declared Once actions, with the
same caching and memory-budgeted scheduling as other builds.

Linux iOS device builds default to ARM64, including on x86_64 hosts. Once uses
the active Swift toolchain's Clang, the SDK's Mach-O linker and archiver,
omarchy-apple-dev's installed resource compatibility tools, and `rcodesign`.
You supply the Apple SDK through xtool; Once does not redistribute it.

A Linux application starter is available through target-kind discovery:

```sh
once edit materialize-example apple_application apple-application-linux
once build Hello
```

The output is ad-hoc signed, not provisioned for device installation or App Store
submission. Simulator execution, Apple test runtimes, Xcode and Swift package
workspace resolution, and Xcode app thinning still require macOS.
See [Build iOS Applications on Linux](/docs/guide/graph/apple/linux) for setup,
SDK repair after toolchain switches, and deployment boundaries.
