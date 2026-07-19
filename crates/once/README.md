# once

Libraries for embedding Once cache access.

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

The high-level programming interface is:

- `Cache`: reusable client bound to the default local cache described by the
  [freedesktop.org base directory specification](https://specifications.freedesktop.org/basedir-spec/latest/).
- Cache selection: `local` for an isolated root and `from_workspace` for the
  same effective provider as the command line.
- Blob operations: in-memory access with `put_blob` and `get_blob`, bounded
  input with `put_stream` and `put_file`, file output with
  `get_blob_to_file`, and existence checks with `has_blob`.
- Action-cache operations: `put_action_result`, `get_action_result`, and `forget_action`.
- Action identity: `ActionKeyBuilder` builds a versioned digest from ordered,
  labeled inputs.
- Cache primitives: `Digest`, `ActionResult`, `Stats`, `CacheProvider`,
  `TuistCacheConfig`, and `digest_from_hex`.

## Apple

Reference `Once.xcframework.zip` from a release with a Swift Package Manager
binary target named `Once`. Then vendor the `Once.swift` wrapper from the
matching release tag into the Swift target that depends on that binary target:

```swift
let once = OnceCache()
let digest = try await once.putBlob(Data("hello".utf8))
let bytes = try await once.getBlob(digest)

print(String(decoding: bytes, as: UTF8.self))
```

If you call the C application programming interface directly, all owned
strings returned by `once_*` functions must be released with
`once_string_free`. The C module name is `Once`.
[JavaScript Object Notation](https://www.json.org/json-en.html) requests omit
cache-selection fields for the default local cache, or provide
`local_cache_root` or `workspace_root`. File operations keep large payloads
out of the host language's memory.

## Ruby

Install the Ruby library from RubyGems:

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

Install the JavaScript library from npm:

```sh
npm install buildonce
```

Use it from Node.js:

```js
const { Cache } = require("buildonce");

async function main() {
  const cache = new Cache();
  const digest = await cache.putBlob("hello");
  const bytes = await cache.getBlob(digest);

  console.log(bytes.toString());
}
```

### Foreign Function Responses

Foreign functions return
[Unicode Transformation Format, 8-bit (UTF-8)](https://www.unicode.org/faq/utf_bom.html#UTF8)
[JavaScript Object Notation](https://www.json.org/json-en.html):

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

The task writes `dist/Once.xcframework.zip` and a
[Secure Hash Algorithm 256-bit](https://csrc.nist.gov/pubs/fips/180-4/upd1/final)
checksum file.
