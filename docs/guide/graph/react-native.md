# React Native

Once models a current [React Native](https://reactnative.dev/) application as
separate dependency, discovery, code generation, JavaScript bundle, native
application, and development-server targets. The bundled starter uses React
Native 0.86 and its
[New Architecture](https://reactnative.dev/architecture/landing-page), with
[Fabric](https://reactnative.dev/architecture/fabric-renderer),
[Hermes](https://reactnative.dev/docs/hermes), and autolinked native modules.

The native package managers remain authoritative. [npm](https://www.npmjs.com/)
installs the exact JavaScript lockfile, [CocoaPods](https://cocoapods.org/)
integrates Apple native dependencies, and [Gradle](https://gradle.org/)
integrates Android native dependencies. Once places their work into explicit
actions and cache keys without replacing their resolution semantics.

## Start from the bundled application

Inspect and materialize the starter:

```sh
once query example react_native_apple_application react-native-application-minimal --output ./HelloReactNative
cd HelloReactNative
once query validate-workspace
```

The same starter contains Apple and Android application targets, so it can
exercise both native projects from one package.

Build the dependency graph and platform applications:

```sh
once build Dependencies
once build OnceBaseline
once build OnceBaselineAndroid
```

Build release JavaScript bundles separately when a distribution workflow needs
them:

```sh
once build AppleBundle
once build AndroidBundle
```

Each bundle is produced by Metro, compiled to Hermes bytecode with the compiler
paired to the locked React Native release, and accompanied by a composed source
map.

## Development and Fast Refresh

Start [Metro](https://metrobundler.dev/) against the live workspace:

```sh
once run Metro
```

In another terminal, launch a cached native application:

```sh
once run OnceBaseline
```

For Android:

```sh
once run OnceBaselineAndroid
```

The Android run capability installs the application package and forwards
Metro's port through the
[Android Debug Bridge](https://developer.android.com/tools/adb). The Apple run
capability installs and launches the application on a booted
[iOS Simulator](https://developer.apple.com/documentation/xcode/running-your-app-in-simulator-or-on-a-device).
Set `adb_serial` when more than one Android device is connected. Once waits for
the selected device to become ready before installation.

JavaScript and TypeScript sources belong to the Metro and bundle targets, not
the native application target. Editing those sources therefore triggers Fast
Refresh without invalidating the cached native application. Native project,
lockfile, module-discovery, or generated-interface changes do invalidate the
relevant native build.

## Native modules

`react_native_autolinking` executes the React Native community
[autolinking](https://github.com/react-native-community/cli/blob/main/docs/autolinking.md)
configuration and emits normalized module metadata. Check that snapshot into
the package when the dependency resolver should expose one
`react_native_module` node per native package.

`react_native_codegen` runs the official
[Codegen](https://reactnative.dev/docs/the-new-architecture/what-is-codegen)
entry point for Apple, Android, or both platforms. Application targets include
the code generation and autolinking action identities in their native build
keys while CocoaPods and Gradle continue to assemble the exact upstream native
projects. The separate generated output is useful for inspection and explicit
validation. CocoaPods and Gradle still regenerate the files they consume.

## Cache boundary

The JavaScript dependency tree is installed and cached once. Consumer targets
stage a lightweight workspace link instead of duplicating that tree in every
target output. Metro bundles, Hermes bytecode, generated native interfaces,
module metadata, and native application products are independently cacheable.
Device installation, application launch, port forwarding, and the Metro server
are runtime effects and always execute.

Native projects are staged from clean sources on an action miss. This prevents
stale generated files from changing results. Unchanged builds use the Once
action cache, while a native source change starts from a clean CocoaPods or
Gradle project rather than reusing an undeclared local build directory.

Generated native directories are excluded by default. Use `exclude_srcs` for
additional project-specific output directories.

Use repeated verbose builds to inspect individual cache decisions:

```sh
once -vv build Dependencies
once -vv build OnceBaseline
```

## Current boundary

- The dependency target supports npm with `package-lock.json`. It does not yet
  support npm workspaces, [pnpm](https://pnpm.io/),
  [Yarn](https://yarnpkg.com/), or [Bun](https://bun.sh/) lockfiles.
- An `npmrc` may configure registries and installation behavior, but it must
  not contain authentication tokens or passwords because declared inputs may
  be stored in a shared cache. Authenticated private registries need a future
  secret-input mechanism.
- Package lifecycle scripts run as part of `npm ci`. Offline mode prevents npm
  registry downloads, but it does not sandbox network or filesystem access
  performed directly by a lifecycle script.
- The first Apple or Android build can still need network access for missing
  CocoaPods sources, a Gradle wrapper distribution, or Gradle artifacts.
- Gradle uses the configured host Gradle cache. Its mutable contents are not
  part of the action key, so the lockfiles and repository checksums remain the
  authority for dependency identity.
- `react_native_bundle` produces a standalone Metro and Hermes artifact for
  distribution workflows. Native release builds continue to use the bundle
  steps owned by their checked-in CocoaPods and Gradle projects.
- Metro watches the application package and its locked dependency snapshot.
  Monorepo packages outside the application package need to be included by a
  project-specific Metro configuration.
- Apple builds accept `iphonesimulator` and `iphoneos`. Once can build a signed
  device product through the checked-in Xcode settings, but `once run` installs
  simulator products only.
- Android variants whose Gradle output does not follow the default
  `app-debug.apk` layout must set `apk_path`.
- React Native actions currently run locally. Remote sandbox execution will
  require a portable dependency materialization strategy instead of workspace
  links.

The native application targets intentionally use the checked-in CocoaPods and
Gradle projects instead of attempting to synthesize those projects. This keeps
current React Native behavior, third-party module scripts, CMake integration,
and Apple prebuilt artifacts compatible while Once makes the important build
and cache boundaries explicit.
