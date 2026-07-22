# `swift_package_pin`

Synthetic locked Swift package identity.

## Description

`swift_package_pin` is emitted by `swift_package_dependencies`. It makes the external package graph visible through the same typed graph and query surfaces as first-party targets. Users should not declare it directly.

Each pin records the package identity, source location, locked state, checkout location, and dependencies on other pins. Source-control packages retain versions, revisions, and branches. Registry packages also retain their checksum. Local packages use a `local` identity suffix.

## Attributes

| Attribute | Type | Required | Description |
| --- | --- | --- | --- |
| `identity` | string | yes | Canonical lowercase Swift package identity |
| `package_name` | string | no | Package display name from the resolved graph |
| `source_kind` | string | no | `remoteSourceControl`, `registry`, or `localSourceControl` |
| `location` | string | no | Registry, source-control, or local package location |
| `version` | string | no | Locked semantic version |
| `revision` | string | no | Locked source-control revision |
| `branch` | string | no | Locked source-control branch |
| `checksum` | string | no | Locked registry checksum |
| `checkout_path` | string | no | Workspace-relative checkout path reported by Swift Package Manager |

## Dependency edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `swift_package_pin` | Locked transitive package dependencies |

## Providers

The target emits `swift_package_pin`. Its provider repeats the lock identity and lists the identities of its direct package dependencies.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | none |

The target has no build action. Compilation is owned by the corresponding `swift_package_dependencies` target.

## Upstream contract

The fields follow Swift Package Manager's [`ResolvedPackagesStore`](https://github.com/swiftlang/swift-package-manager/blob/main/Sources/PackageGraph/ResolvedPackagesStore.swift) model.
