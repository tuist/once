# React Native with Once

This starter is a bare [React Native](https://reactnative.dev/) 0.86
application using the
[New Architecture](https://reactnative.dev/architecture/landing-page),
[Hermes](https://reactnative.dev/docs/hermes), and
[Fast Refresh](https://reactnative.dev/docs/fast-refresh).

Install and cache the locked JavaScript packages:

```sh
once build Dependencies
```

Build the native applications:

```sh
once build OnceBaseline
once build OnceBaselineAndroid
```

Start [Metro](https://metrobundler.dev/) in one terminal:

```sh
once run Metro
```

Launch either native application from another terminal:

```sh
once run OnceBaseline
once run OnceBaselineAndroid
```

Edit `App.tsx` while Metro is running to see the application update through
Fast Refresh. JavaScript-only edits do not invalidate either native build.

The Apple build uses [CocoaPods](https://cocoapods.org/) for native
dependencies. The Android build uses [Gradle](https://gradle.org/). Once keeps
those upstream integrations and adds typed targets, explicit action inputs,
and independent cache keys around them.
