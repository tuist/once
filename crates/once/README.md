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

- `Cache`: reusable client bound to the default XDG local cache.
- Blob operations: `put_blob`, `get_blob`, and `has_blob`.
- Action-cache operations: `put_action_result`, `get_action_result`, and `forget_action`.
- Cache primitives: `Digest`, `ActionResult`, `Stats`, `CacheProvider`, and `digest_from_hex`.

## Apple

Reference `Once.xcframework.zip` from a release with a SwiftPM binary
target named `Once`, then add `crates/once/swift/Once.swift` from the
matching repository tag to the Swift target that depends on that binary
target:

```swift
let once = OnceCache()
let digest = try await once.putBlob(Data("hello".utf8))
let bytes = try await once.getBlob(digest)

print(String(decoding: bytes, as: UTF8.self))
```

If you call the C API directly, all owned strings returned by `once_*`
functions must be released with `once_string_free`. The C module name is
`Once`. JSON requests use the default XDG local cache.

## Ruby

Install the Ruby SDK from RubyGems:

```sh
gem install buildonce
```

Use it to read and write blobs through the default local cache:

```ruby
require "buildonce"

cache = Once::Cache.new
digest = cache.put_blob("hello")
bytes = cache.get_blob(digest)

puts bytes
```

## JavaScript

Install the JavaScript SDK from npm:

```sh
npm install @tuist/once
```

Use it from Node.js:

```js
const { Cache } = require("@tuist/once");

async function main() {
  const cache = new Cache();
  const digest = await cache.putBlob("hello");
  const bytes = await cache.getBlob(digest);

  console.log(bytes.toString());
}
```

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

The task writes `dist/Once.xcframework.zip` and a SHA-256 file.
