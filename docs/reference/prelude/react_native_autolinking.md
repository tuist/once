# `react_native_autolinking`

Normalized React Native native-module discovery.

## Description

Executes the React Native community
[autolinking](https://github.com/react-native-community/cli/blob/main/docs/autolinking.md)
configuration and converts absolute project paths into a portable,
application-scoped JavaScript Object Notation snapshot.

## Attributes

`node` selects the Node.js executable and defaults to `node`. The `deps` edge
accepts exactly one `react_native_dependency_set`.

## Capabilities

The `build` capability exposes `default` and `modules_snapshot`.

