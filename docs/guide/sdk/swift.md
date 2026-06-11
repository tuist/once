# Swift SDK

The Swift SDK is a thin async wrapper over the C API in the
`Once.xcframework` release artifact. It exposes cache primitives for Apple
platforms. Script execution is CLI specific and is not part of the Swift
SDK surface.

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
SwiftPM binary target:

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

Replace the version in the URL with the Once release you want to use. The
checksum is published next to the release asset and can also be computed
locally with `swift package compute-checksum Once.xcframework.zip`.

Vendor the `Once.swift` wrapper that ships with the matching release tag
into the Swift target that depends on `Once`. The wrapper imports the C
module from the binary target and gives callers the high-level Swift API.

## OnceCache

`OnceCache` is the Swift cache client.

`OnceCache` opens the default local cache using XDG conventions.

All cache operations that touch storage are `async throws`.

| API | Use |
| --- | --- |
| `init()` | Opens the default local cache using XDG conventions. |
| `version` | Returns the linked Once version. |

The default cache root is `$XDG_CACHE_HOME/once/cas` when
`XDG_CACHE_HOME` is set, and `$HOME/.cache/once/cas` otherwise.

## Blobs

Blobs are content-addressed byte payloads. Store bytes once, then refer to
them by digest from action results, manifests, or other integration state.

| API | Use |
| --- | --- |
| `digest(bytes:)` | Returns the content digest for bytes without writing them to the cache. |
| `putBlob(_:)` | Stores bytes and returns their content digest. |
| `getBlob(_:)` | Reads bytes for a digest. |
| `hasBlob(_:)` | Returns whether a blob exists. |

## Action Results

Action results let embedders associate an action digest with an exit code,
stdout digest, stderr digest, and output digests. The Swift SDK stores and
retrieves metadata for completed actions. It does not run commands.

| API | Use |
| --- | --- |
| `putActionResult(_:for:)` | Stores a cached result for an action digest. |
| `getActionResult(_:)` | Returns a cached result when one exists. |
| `forgetAction(_:)` | Removes one cached action result. Referenced blobs are left intact. |
| `stats()` | Returns local cache statistics. |

## Types

These are the public supporting types exposed by the Swift wrapper. They
let embedders model cache state without calling the C API directly.

| Type | Use |
| --- | --- |
| `OnceActionResult` | Stores an action exit code, stdout digest, stderr digest, and output digests. |
| `OnceCacheStats` | Reports local cache size and entry counts. |
| `OnceError` | Reports Swift wrapper failures. |
