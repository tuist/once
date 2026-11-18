# React Native with Struo

This starter is a bare [React Native](https://reactnative.dev/) 0.86
application using the
[New Architecture](https://reactnative.dev/architecture/landing-page),
[Hermes](https://reactnative.dev/docs/hermes), and
[Fast Refresh](https://reactnative.dev/docs/fast-refresh).

Install and cache the locked JavaScript packages:

```sh
struo build Dependencies
```

Build the small TypeScript library and its declarations with a package script:

```sh
struo build Library
```

This target uses `javascript_build` with the same locked installation. Its
JavaScript and declaration products are separate from the native applications.

Build the native applications:

```sh
struo build StruoBaseline
struo build StruoBaselineAndroid
```

Start [Metro](https://metrobundler.dev/) in one terminal:

```sh
struo run Metro
```

Launch either native application from another terminal:

```sh
struo run StruoBaseline
struo run StruoBaselineAndroid
```

Edit `App.tsx` while Metro is running to see the application update through
Fast Refresh. JavaScript-only edits do not invalidate either native build.

The Apple build uses [CocoaPods](https://cocoapods.org/) for native
dependencies. The Android build uses [Gradle](https://gradle.org/). Struo keeps
those upstream integrations and adds typed targets, explicit action inputs,
and independent cache keys around them.
