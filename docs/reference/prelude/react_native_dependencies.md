# `react_native_dependencies`

Locked JavaScript dependencies for React Native.

## Description

Reads `package.json`, `package-lock.json`, and an optional normalized module
snapshot during graph loading. The resolver records exact React Native and
Hermes versions, emits one `react_native_module` target per snapshotted native
package, and runs [`npm ci`](https://docs.npmjs.com/cli/commands/npm-ci) as a
cacheable action.

The installed dependency tree is the shared input for Metro, Codegen,
autolinking, and native application targets.

Include repository-local `file:` packages in `srcs`. Once stages those files
beside the package manifest before installation, so npm creates valid local
links and changes to those packages invalidate the dependency action.

An optional `npmrc` may contain non-secret registry and installation settings.
It must be present in `resolver_inputs` and must not contain authentication
tokens or passwords. Once rejects credential-bearing configuration because
declared inputs may be uploaded to a shared cache.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `package_json` | string | no | `package.json` | JavaScript package manifest |
| `lockfile` | string | no | `package-lock.json` | npm lockfile |
| `npmrc` | string | no |  | Optional npm configuration without credentials |
| `modules_snapshot` | string | no |  | Normalized native-module snapshot |
| `resolver_inputs` | list&lt;string&gt; | no | `[]` | Text inputs available to the resolver |
| `node` | string | no | `node` | Node.js executable |
| `npm` | string | no | `npm` | npm executable |
| `allow_network` | bool | no | `false` | Permit package downloads missing from the local cache |
| `install_args` | list&lt;string&gt; | no | `[]` | Additional `npm ci` arguments |

Underscore-prefixed attributes are resolver-owned and must not be set by a
package manifest.

## Providers and capabilities

The target emits `javascript_dependency_set` and
`react_native_dependency_set`. Its `build` capability exposes
`node_modules`.

This target currently supports npm with `package-lock.json`. npm workspaces and
pnpm, Yarn, or Bun lockfiles are not supported. Package lifecycle scripts run
during installation. `allow_network = false` prevents npm registry downloads,
but it does not sandbox direct network or filesystem access by those scripts.

## Example

```toml
[[target]]
name = "Dependencies"
kind = "react_native_dependencies"
srcs = [
  "package.json",
  "package-lock.json",
  "react-native-modules.json",
  "packages/native-camera/**/*",
]

[target.attrs]
modules_snapshot = "react-native-modules.json"
resolver_inputs = ["package.json", "package-lock.json", "react-native-modules.json"]
```
