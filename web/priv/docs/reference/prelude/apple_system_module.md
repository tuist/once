# `apple_system_module`

Authored Clang system module exposed to Apple consumers.

## Description

`apple_system_module` exposes a module map and its headers without compiling a
library archive. Dependent Swift and C-family targets receive the module map
at compile time, and final Apple binaries link the declared system libraries.
This is useful for Swift Package Manager `system` targets that wrap headers
provided by the operating system.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `modulemap` | string | yes |  | Workspace-relative authored Clang module map |
| `headers` | list&lt;string&gt; | no | `[]` | Module-map headers tracked as compiler inputs |
| `header_dirs` | list&lt;string&gt; | no | `[]` | Additional exported header search directories |
| `sdk_dylibs` | list&lt;string&gt; | no | `[]` | System libraries linked by dependent Apple binaries |
| `linkopts` | list&lt;string&gt; | no | `[]` | Additional linker flags propagated to dependent Apple binaries |

## Providers and capabilities

The target emits `apple_linkable` and `apple_module`, so Apple libraries,
frameworks, applications, and test bundles can use it through their ordinary
`deps` edges.

| Capability | Output groups |
| --- | --- |
| `build` | `default`, `modulemap` |
