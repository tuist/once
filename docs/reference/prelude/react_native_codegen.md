# `react_native_codegen`

React Native New Architecture code generation.

## Description

Runs the official React Native
[Codegen](https://reactnative.dev/docs/the-new-architecture/what-is-codegen)
entry point against application and native-module specifications.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `platform` | string | yes |  | `ios`, `android`, or `all` |
| `node` | string | no | `node` | Node.js executable |

The `deps` edge accepts exactly one `react_native_dependency_set`. The `build`
capability exposes `default` and `generated_sources`.

