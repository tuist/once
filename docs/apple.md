# Apple and iOS

Fabrik supports a first-pass `apple.ios_app` target for Swift iOS simulator apps.

```toml
[[apple.ios_app]]
name = "Demo"
bundle_id = "dev.fabrik.ios-demo"
srcs = ["Sources/App.swift"]
minimum_os = "17.0"
```

Build the app bundle:

```sh
fabrik build //examples/apple/ios/simulator-app:Demo
```

Launch it in a simulator:

```sh
fabrik run //examples/apple/ios/simulator-app:Demo
```

Set `FABRIK_IOS_SIMULATOR` to a simulator UDID when you do not want to use the booted device:

```sh
FABRIK_IOS_SIMULATOR=<udid> fabrik run //examples/apple/ios/simulator-app:Demo
```

## Cache Behavior

- `fabrik build` for `apple.ios_app` is cacheable.
- `fabrik run` first reuses the cacheable app build, then runs an uncached install and launch action.
- Simulator boot, install, and launch are runtime side effects and are intentionally not cached.

## Current Limits

- The current implementation targets iOS simulator builds.
- App dependencies are not wired yet.
- The build action is coarse. Future Apple support should split Swift compilation, asset work, signing, packaging, install, and launch into separate tasks where practical.
