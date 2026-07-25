# React Native

Once models a current [React Native](https://reactnative.dev/) application as
separate dependency, discovery, code generation, JavaScript bundle, native
application, and development-server targets. The bundled starter uses React
Native 0.86 and its
[New Architecture](https://reactnative.dev/architecture/landing-page), with
[Fabric](https://reactnative.dev/architecture/fabric-renderer),
[Hermes](https://reactnative.dev/docs/hermes), and autolinked native modules.

The package managers remain authoritative.
[npm](https://www.npmjs.com/), [pnpm](https://pnpm.io/),
[Yarn](https://yarnpkg.com/), or [Bun](https://bun.sh/) installs the exact
JavaScript lockfile. [CocoaPods](https://cocoapods.org/) integrates Apple
native dependencies, and [Gradle](https://gradle.org/) integrates Android
native dependencies. Once places their work into explicit actions and cache
keys without replacing their resolution semantics.

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

## Boundaries

### Package installation

Once supports `package-lock.json`, `pnpm-lock.yaml`, `yarn.lock`, and
`bun.lock`. It detects the manager from the target attributes, the
`packageManager` field, or the declared lockfile. The dependency target models
one application package, so workspace-root installation is not yet
synthesized. Package-manager configuration may be declared as an input, but it
must not contain credentials because inputs can enter a shared cache.
Lifecycle scripts still run with the package manager's normal permissions.

### Native builds

The application targets use the checked-in CocoaPods and Gradle projects.
This preserves upstream React Native behavior and native-module integration.
A first build may still fetch missing CocoaPods sources, a Gradle distribution,
or Gradle artifacts. Native release builds use the bundling steps owned by
those projects, while `react_native_bundle` produces a separate Metro and
Hermes artifact for distribution workflows.

### Development and execution

Metro watches the application package and its locked dependencies. Packages
outside that boundary require a project-specific Metro configuration. Once
can build Apple simulator or signed-device products, but `once run` installs
simulator products only. React Native actions currently run locally because
remote execution needs a portable replacement for workspace dependency links.
