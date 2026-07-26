# `react_native_dependencies`

Locked JavaScript dependencies for React Native.

## Description

Reads `package.json`, one supported lockfile, and an optional normalized module
snapshot during graph loading. The resolver records exact React Native and
Hermes versions, emits one `react_native_module` target per snapshotted native
package, and runs a frozen package-manager install as a cacheable action.

| Package manager | Lockfile | Frozen install |
| --- | --- | --- |
| [npm](https://www.npmjs.com/) | `package-lock.json` | [`npm ci`](https://docs.npmjs.com/cli/commands/npm-ci) |
| [pnpm](https://pnpm.io/) | `pnpm-lock.yaml` | [`pnpm install --frozen-lockfile`](https://pnpm.io/cli/install) |
| [Yarn](https://yarnpkg.com/) | `yarn.lock` | [`--frozen-lockfile`](https://classic.yarnpkg.com/lang/en/docs/cli/install/) for Yarn 1, [`--immutable`](https://yarnpkg.com/cli/install) for modern Yarn |
| [Bun](https://bun.sh/) | `bun.lock` | [`bun install --frozen-lockfile`](https://bun.sh/docs/pm/cli/install) |

Set `package_manager` explicitly or leave it as `auto`. Automatic selection
uses an explicit lockfile path first, then `package.json`'s `packageManager`
field, then the only supported lockfile supplied to the resolver.

Modern Yarn installs with its `node-modules` linker. pnpm public-hoists only
`@react-native/*`, which makes the React Native Gradle plugin available at the
conventional path expected by checked-in Android projects. Bun uses its
hoisted linker.

The installed dependency tree is the shared input for Metro, Codegen,
autolinking, and native application targets.

Include repository-local `file:` packages in `srcs`. Once stages those files
beside the package manifest before installation, so the selected manager
creates valid local links and changes to those packages invalidate the
dependency action.

Use `package_manager_files` for text configuration such as `.npmrc` or
`.yarnrc.yml`. Each file must also be present in `resolver_inputs`.
Configuration must not contain authentication tokens or passwords because
declared inputs may be uploaded to a shared cache. Include binary manager files
such as an offline package archive in `srcs`, not `resolver_inputs`.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `package_json` | string | no | `package.json` | JavaScript package manifest |
| `package_manager` | string | no | `auto` | `auto`, `npm`, `pnpm`, `yarn`, or `bun` |
| `package_manager_executable` | string | no |  | Package manager executable override |
| `lockfile` | string | no |  | Lockfile path, detected when omitted |
| `npmrc` | string | no |  | Optional npm configuration without credentials |
| `package_manager_files` | list&lt;string&gt; | no | `[]` | Additional text configuration without credentials |
| `modules_snapshot` | string | no |  | Normalized native-module snapshot |
| `resolver_inputs` | list&lt;string&gt; | no | `[]` | Text inputs available to the resolver |
| `node` | string | no | `node` | Node.js executable |
| `allow_network` | bool | no | `false` | Permit package downloads missing from the local cache |
| `install_args` | list&lt;string&gt; | no | `[]` | Additional frozen-install arguments |

Underscore-prefixed attributes are resolver-owned and must not be set by a
package manifest.

## Providers and capabilities

The target emits `javascript_dependency_set` and
`react_native_dependency_set`. Its `build` capability exposes
`node_modules`.

Package lifecycle scripts run during installation. `allow_network = false`
uses each manager's offline mode and cache, but it does not sandbox direct
network or filesystem access by lifecycle scripts. Bun currently offers a
prefer-offline mode rather than a strict offline install, so a cache miss may
still reach the registry.

This target models one application package and does not synthesize package
manager workspaces. Switching package managers can also change native podspec
checksums, so regenerate and check in `Podfile.lock` after a switch.

## Example

```toml
[[target]]
name = "Dependencies"
kind = "react_native_dependencies"
srcs = [
  "package.json",
  "pnpm-lock.yaml",
  "react-native-modules.json",
  "packages/native-camera/**/*",
]

[target.attrs]
package_manager = "pnpm"
lockfile = "pnpm-lock.yaml"
modules_snapshot = "react-native-modules.json"
resolver_inputs = ["package.json", "pnpm-lock.yaml", "react-native-modules.json"]
```
