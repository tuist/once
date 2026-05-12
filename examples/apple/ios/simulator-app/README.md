# iOS App

This example represents a first-pass SwiftUI iOS simulator app bundle.

It uses `apple.simulator_app` to compile Swift sources, assemble an `.app`
bundle, write `Info.plist`, sign the bundle, and launch it through the
simulator flow when run.

Run it from the repository root:

```sh
fabrik build examples/apple/ios/simulator-app/Demo
fabrik run examples/apple/ios/simulator-app/Demo
```
