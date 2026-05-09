# iOS App

This example represents a first-pass SwiftUI iOS simulator app bundle.

It uses `apple.ios_app` to compile Swift sources, assemble an `.app`
bundle, write `Info.plist`, sign the bundle, and launch it through the
simulator flow when run.

Run it from the repository root:

```sh
mise exec -- target/release/fabrik build //examples/apple/ios/simulator-app:Demo
mise exec -- target/release/fabrik run //examples/apple/ios/simulator-app:Demo
```
