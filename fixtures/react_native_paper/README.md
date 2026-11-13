# React Native Paper control

This manifest overlays callstack/react-native-paper at
`e771fcef2ffc9bbb0d28e5aac7ce96f6771e0848`. Copy it into the root of a
checkout of that revision. It does not vendor Paper or change its package
manifests, Yarn configuration, or lockfile.

Use Node.js compatible with Paper's pinned version and Corepack's Yarn 4.9.1.
Enable the Yarn shim on `PATH` before invoking Once, including with `once -C`.

Native package-manager controls:

```sh
corepack yarn install --immutable
corepack yarn prepack
CI=1 corepack yarn --cwd example run expo export --platform all --no-bytecode --max-workers 2 --output-dir dist
```

Once controls:

```sh
once build Dependencies
once build Paper
once build Example
```

`Dependencies` only installs the full workspace. `Paper` builds the library's
modules, TypeScript declarations, and mappings into `lib`. `Example` exports
Android, iOS, and web JavaScript bundles and assets into `example/dist`.
Unlike Paper's default CI export, it deliberately disables Hermes compilation:
this pinned Expo release embeds a random temporary filename in bytecode,
making otherwise identical exports differ on every execution. The default
upstream export was also tested successfully, but is not a reproducible output
control. This fixture does not compile an APK or Apple application.
Native application compilation needs Expo-generated native projects and the
platform toolchain, which this manifest does not synthesize.

The dependency declaration includes both workspace manifests, Yarn's
configuration and checked-in release, and local lifecycle scripts. Frozen
installs remain enabled. The consumer targets declare build sources
separately, preserving nested installs and workspace links in private copies.
Deleting `.once/out` and rebuilding exercises cache restoration without
changing the native source checkout. Large workspaces need enough disk space
for the installation, private consumer trees, and atomic cache restoration.
