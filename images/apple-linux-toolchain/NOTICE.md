# Once Apple Linux toolchain notices

This image intentionally excludes all Apple software. In particular, it does
not contain Xcode, Apple software development kits, Apple framework headers or
libraries, simulators, or executables extracted from an Apple download.

Included components:

- Swift.org Linux toolchain, licensed under Apache License 2.0 with the
  Runtime Library Exception. See <https://www.swift.org/legal/license.html>.
- [Low Level Virtual Machine](https://llvm.org/) and Clang packages, licensed
  under Apache License 2.0 with Low Level Virtual Machine exceptions. See
  <https://llvm.org/docs/DeveloperPolicy.html>.
- [viraptor/actool](https://github.com/viraptor/actool), licensed under the
  [MIT License](https://opensource.org/license/mit).
- [viraptor/ibtool](https://github.com/viraptor/ibtool), licensed under the
  [MIT License](https://opensource.org/license/mit).
- [apple-codesign](https://crates.io/crates/apple-codesign), licensed under the
  [Mozilla Public License 2.0](https://www.mozilla.org/en-US/MPL/2.0/).
- [apple-plist](https://crates.io/crates/apple-plist), licensed under Apache
  License 2.0. It is used by the Once `plutil` compatibility command, which is
  licensed under this repository’s MIT License.
- Ubuntu packages and their transitive dependencies. Their installed copyright
  notices remain available under `/usr/share/doc` in the image.

The publication workflow attaches a software bill of materials and provenance
to each image. This notice does not replace the notices required by transitive
dependencies.
