# once

Embeddable SDK for Once cache access.

## Rust

Add the crate from this repository:

```toml
[dependencies]
once = { git = "https://github.com/tuist/once" }
```

Read and write blobs with a local cache:

```rust
#[tokio::main]
async fn main() -> once::Result<()> {
    let cache = once::Cache::new();
    let digest = cache.put_blob(b"hello").await?;
    let bytes = cache.get_blob(digest).await?;

    assert_eq!(bytes, b"hello");
    Ok(())
}
```

The high-level API is:

- `Cache`: reusable client bound to the default XDG local cache, or an explicit cache provider.
- Blob operations: `put_blob`, `get_blob`, and `has_blob`.
- Action-cache operations: `put_action_result`, `get_action_result`, and `forget_action`.
- Metadata helpers: `stats`, `digest_from_hex`, and lower-level digest/action types.

Lower-level modules are re-exported as `once::core`, `once::cas`, and
`once::frontend` for integrations that need action construction, CAS
access, or manifest parsing.

## Apple

Download `Once.xcframework.zip` from a release, link `Once.xcframework`,
and add the included `Once.swift` file to your Swift target. `Once.swift`
wraps the `COnce` C module:

```swift
let once = OnceCache()
let digest = try await once.putBlob(Data("hello".utf8))
let bytes = try await once.getBlob(digest)

print(String(decoding: bytes, as: UTF8.self))
```

If you call the C API directly, all owned strings returned by `once_*`
functions must be released with `once_string_free`. The C module name is
`COnce`. JSON requests use the default XDG local cache unless they include
`local_cache_root` as an override.

### FFI responses

FFI functions return UTF-8 JSON:

```json
{ "status": "ok", "value": "..." }
```

or:

```json
{ "status": "error", "message": "..." }
```

`once_cache_get_blob_json` returns this value on success:

```json
{
  "bytes": [104, 101, 108, 108, 111]
}
```

### Building Locally

On macOS with Xcode installed:

```sh
mise run release:package-xcframework --version 0.0.0
```

The task writes `dist/Once-0.0.0.xcframework.zip` and a SHA-256 file.
