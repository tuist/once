# Swift SDK

The Swift SDK is a thin async wrapper over the C API in the
`Once.xcframework` release artifact. It exposes cache primitives for Apple
platforms. Script execution is CLI specific and is not part of the Swift
SDK surface.

```swift
import Foundation
import COnce

let cache = OnceCache()
let digest = try await cache.putBlob(Data("hello".utf8))
let bytes = try await cache.getBlob(digest)
```

## Installation

Download `Once.xcframework.zip` from a Once release and link the
`Once.xcframework` artifact from your application or package.

Add `crates/once/swift/Once.swift` to the Swift target that links the
framework. The wrapper imports the C module as `COnce`.

Applications that use `Once.swift` directly import `COnce` from the linked
XCFramework and compile the Swift wrapper into their own target.

## OnceCache

`OnceCache` is the Swift cache client.

`init()` opens the default local cache using XDG conventions:

- `$XDG_CACHE_HOME/once/cas` when `XDG_CACHE_HOME` is set
- `$HOME/.cache/once/cas` otherwise

`init(localCacheRoot:)` opens a local filesystem cache at an explicit root.
Use this for tests, isolated sandboxes, and applications that need a
caller-owned cache location.

`version` returns the linked Once version.

## Blobs

`digest(bytes:) async throws -> String` returns the content digest for
bytes without writing them to the cache.

`putBlob(_:) async throws -> String` stores bytes and returns their
content digest.

`getBlob(_:) async throws -> Data` reads bytes for a digest.

`hasBlob(_:) async throws -> Bool` returns whether a blob exists.

## Action Results

`actionDigest(actionJSON:) async throws -> String` returns the digest for a
canonical action JSON payload.

`putActionResult(_:for:) async throws -> Bool` stores a cached result for
an action digest.

`getActionResult(_:) async throws -> OnceActionResult?` returns a cached
result when one exists.

`forgetAction(_:) async throws -> Bool` removes one cached action result
and returns whether a result was removed. Referenced blobs are left
intact.

`stats() async throws -> OnceCacheStats` returns local cache statistics.

## Types

`OnceActionResult` stores the cached result of an action:

- `exitCode: Int32`
- `stdout: String?`
- `stderr: String?`
- `outputs: [String: String]`

`OnceCacheStats` reports cache size and entry counts:

- `blobCount: UInt64`
- `blobBytes: UInt64`
- `actionCount: UInt64`
- `actionBytes: UInt64`

`OnceError` reports Swift wrapper failures:

- `invalidUTF8`
- `api(String)`
