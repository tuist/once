---
prev: false
next: false
---

# Swift Software Development Kit

The Swift library is a thin asynchronous wrapper over the C application
programming interface in the `Once.xcframework` release artifact. It exposes
cache primitives for Apple
platforms. Script execution belongs to the command line and is not part of
this library. See the [language-library overview](/guide/sdk/) to compare the
available bindings.

```swift
import Foundation
import Once

func example() async throws {
    let cache = OnceCache()
    let digest = try await cache.putBlob(Data("hello".utf8))
    let bytes = try await cache.getBlob(digest)

    assert(bytes == Data("hello".utf8))
}
```

## Installation

Reference the released XCFramework from your package manifest with a
[Swift Package Manager](https://www.swift.org/documentation/package-manager/)
binary target:

```swift
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "MyPackage",
    products: [
        .library(name: "MyPackage", targets: ["MyPackage"]),
    ],
    targets: [
        .target(
            name: "MyPackage",
            dependencies: ["Once"]
        ),
        .binaryTarget(
            name: "Once",
            url: "https://github.com/tuist/once/releases/download/0.1.0/Once.xcframework.zip",
            checksum: "<checksum>"
        ),
    ]
)
```

Replace the version in the download address with the Once release you want to use. The
checksum is published next to the release asset and can also be computed
locally with `swift package compute-checksum Once.xcframework.zip`.

Vendor the `Once.swift` wrapper that ships with the matching release tag
into the Swift target that depends on `Once`. The wrapper imports the C
module from the binary target and gives callers the high-level Swift interface.

## OnceCache

`OnceCache` is the Swift cache client.

`OnceCache` opens the default local cache using the
[X Desktop Group base-directory convention](https://specifications.freedesktop.org/basedir-spec/latest/).

All cache operations that touch storage are `async throws`.

| Application programming interface | Use |
| --- | --- |
| `init()` | Opens the default local cache using the operating-system convention. |
| `version` | Returns the linked Once version. |

The default cache root is `$XDG_CACHE_HOME/once/cas` when
`XDG_CACHE_HOME` is set, and `$HOME/.cache/once/cas` otherwise.

## Blobs

Blobs are content-addressed byte payloads. Store bytes once, then refer to
them by digest from action results, manifests, or other integration state.

| Application programming interface | Use |
| --- | --- |
| `digest(bytes:)` | Returns the content digest for bytes without writing them to the cache. |
| `putBlob(_:)` | Stores bytes and returns their content digest. |
| `getBlob(_:)` | Reads bytes for a digest. |
| `hasBlob(_:)` | Returns whether a blob exists. |

## Action Results

Action results let embedders associate an action digest with an exit code,
stdout digest, stderr digest, and output digests. The Swift library stores and
retrieves metadata for completed actions. It does not run commands.

| Application programming interface | Use |
| --- | --- |
| `putActionResult(_:for:)` | Stores a cached result for an action digest. |
| `getActionResult(_:)` | Returns a cached result when one exists. |
| `forgetAction(_:)` | Removes one cached action result. Referenced blobs are left intact. |
| `stats()` | Returns local cache statistics. |

## Types

These are the public supporting types exposed by the Swift wrapper. They
let embedders model cache state without calling the C application programming
interface directly.

| Type | Use |
| --- | --- |
| `OnceActionResult` | Stores an action exit code, stdout digest, stderr digest, and output digests. |
| `OnceCacheStats` | Reports local cache size and entry counts. |
| `OnceError` | Reports Swift wrapper failures. |
