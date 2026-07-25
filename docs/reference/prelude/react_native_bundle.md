# `react_native_bundle`

Metro and Hermes release bundle.

## Description

Runs [Metro](https://metrobundler.dev/) for one platform and optionally compiles
its output with the [Hermes](https://reactnative.dev/docs/hermes) compiler
paired to the locked React Native version. Hermes source maps are composed with
Metro source maps into one final map.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `platform` | string | yes |  | `ios` or `android` |
| `entry` | string | no | `index.js` | JavaScript entry file |
| `bundle_name` | string | no | `main.jsbundle` | Final bundle filename |
| `metro_config` | string | no |  | Package-relative Metro configuration |
| `dev` | bool | no | `false` | Generate a development bundle |
| `minify` | bool | no | `true` | Minify Metro output |
| `hermes` | bool | no | `true` | Compile to Hermes bytecode |
| `bundle_args` | list&lt;string&gt; | no | `[]` | Additional Metro arguments |
| `node` | string | no | `node` | Node.js executable |

The `deps` edge accepts exactly one `react_native_dependency_set`.

On macOS, x86-64 Linux, and x86-64 Windows, Once resolves the Hermes compiler
through the locked React Native package. This supports both hoisted and
isolated dependency layouts. Other hosts fail during analysis with a clear
unsupported-host message.

## Capabilities

The `build` capability exposes `default`, `bundle`, `source_map`, and `assets`.
