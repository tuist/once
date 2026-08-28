# Once Apple Linux toolchain

This is a redistributable [Open Container Initiative image](https://opencontainers.org/)
of Linux-native tools used by Once Apple build actions. It has no Xcode,
Apple software development kits, simulators, frameworks, headers, libraries,
or executables extracted from Apple downloads.

It is a tool layer, not an Xcode replacement. The published image can be used
as the Linux executor base before an authorized platform input is mounted by a
separate integration.

## Included tools

| Capability | Command | Implementation and scope |
| --- | --- | --- |
| Swift compilation | `swift`, `swiftc` | Swift.org Linux toolchain |
| C and C++ compilation | `clang`, `clang++` | [Low Level Virtual Machine](https://llvm.org/) compiler tools |
| Apple-format linking and archive creation | `ld64.lld`, `llvm-ar`, `llvm-lipo`, `llvm-libtool-darwin`, `llvm-ranlib` | Low Level Virtual Machine tools |
| Property-list conversion | `plutil` | Once compatibility command for `json` ([JavaScript Object Notation](https://www.json.org/json-en.html)), `xml1`, and `binary1` conversion |
| Asset catalogs | `actool` | [viraptor/actool](https://github.com/viraptor/actool), a clean-room Rust implementation |
| Interface resources | `ibtool` | [viraptor/ibtool](https://github.com/viraptor/ibtool), experimental and limited to macOS interface resources |
| Code signing | `rcodesign` | [apple-codesign](https://crates.io/crates/apple-codesign), a Rust command with its own command-line interface |

`rcodesign` is deliberately not exposed as `codesign`: its behavior and
arguments are not identical to Apple’s command. The Once target kind must
choose it explicitly in the follow-up integration.

## Unsupported Xcode capabilities

The image fails closed for these capabilities, because there is not a
validated Linux-native replacement in this release:

- Core Data model compilation (`momc`)
- Intent-definition compilation (`intentbuilderc`)
- Entitlement record generation (`derq`)
- Device-specific application thinning (`ipatool`)
- Xcode tool discovery (`xcrun`), simulator management, and `xcodebuild`

## Platform input boundary

Apple platform content must be supplied outside this image, subject to the
applicable Apple license. A software development kit sysroot can be enough for
some C and Objective-C targets. Swift targets that import Apple platform
modules can additionally require target-specific Swift modules, runtimes, and
platform libraries. The later Once integration will model those inputs
explicitly and will not copy them into a public image.

## Publishing and verification

The GitHub Actions workflow builds and verifies the Linux x86-64 image for
pull requests. On the main branch, releasable conventional commits produce a
versioned multi-architecture image in GitHub Container Registry and a matching
GitHub Release. The initial release is published even if its bootstrap commit
does not use a releasable conventional-commit type.

Each published release has these image tags:

- `ghcr.io/tuist/once-apple-linux-toolchain:<version>`
- `ghcr.io/tuist/once-apple-linux-toolchain:sha-<commit>`
- `ghcr.io/tuist/once-apple-linux-toolchain:latest`

Run the in-image policy and tool check with:

`@BT@`sh
docker run --rm ghcr.io/tuist/once-apple-linux-toolchain:<version> \
  once-apple-linux-toolchain-verify
`@BT@`
