# `react_native_metro`

Live Metro development server for React Native.

## Description

Runs [Metro](https://metrobundler.dev/) against live workspace sources while
resolving packages from the locked dependency target. The action is
non-cacheable and remains attached to the source tree so React Native Fast
Refresh observes JavaScript and TypeScript edits without a native rebuild.
The user Metro configuration is loaded from the live package, so it can import
sibling configuration files. Once keeps a private locked dependency snapshot
for the lifetime of the server so another dependency build cannot invalidate
the running watch process.

The default watch roots cover the application package and the locked dependency
snapshot. A monorepo can add sibling package roots through its Metro
configuration.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `port` | int | no | `8081` | Development server port |
| `host` | string | no | `127.0.0.1` | Development server host |
| `metro_config` | string | no |  | User Metro configuration merged with Once paths |
| `reset_cache` | bool | no | `false` | Clear Metro's transform cache at startup |
| `node` | string | no | `node` | Node.js executable |

The `deps` edge accepts exactly one `react_native_dependency_set`. The target
exposes the `run` capability.
