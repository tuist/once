# Rust SDK

The Rust SDK is the `once` crate. It exposes cache primitives for
embedding Once in Rust applications and tools. Script execution is CLI
specific and is not part of the SDK surface.

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

## Installation

Add the crate to your `Cargo.toml`:

```toml
[dependencies]
once = { git = "https://github.com/tuist/once" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

The crate is released from this repository for source integration. It is
not currently published to the crates.io registry.

## Cache

`once::Cache` is the high-level cache client. It is cheap to clone and can
be reused across blob and action-result operations.

### Constructors

| API | Use |
| --- | --- |
| `Cache::new()` | Opens the default local cache using XDG conventions. |
| `Cache::local(local_cache_root)` | Opens a local filesystem cache at an explicit root. |
| `Cache::with_provider(cache)` | Wraps an existing `CacheProvider`. |

The default cache root is `$XDG_CACHE_HOME/once/cas` when
`XDG_CACHE_HOME` is set, and `$HOME/.cache/once/cas` otherwise. Use
`Cache::local(...)` for tests, isolated sandboxes, and applications that
need a caller-owned cache location.

### Introspection

| API | Use |
| --- | --- |
| `provider()` | Returns the underlying `CacheProvider`. |
| `root()` | Returns the local root directory used by the provider. |

### Blobs

| API | Use |
| --- | --- |
| `put_blob(bytes)` | Stores bytes and returns their content digest. |
| `get_blob(digest)` | Reads bytes for a digest. |
| `has_blob(digest)` | Returns whether a blob exists. |

### Action Results

Action results let embedders associate an action digest with an exit code,
stdout digest, stderr digest, and output digests.

```rust
use std::collections::BTreeMap;

#[tokio::main]
async fn main() -> once::Result<()> {
    let cache = once::Cache::new();
    let stdout = cache.put_blob(b"compiled").await?;
    let action = once::Digest::of_bytes(br#"{"tool":"example"}"#);
    let result = once::ActionResult {
        exit_code: 0,
        stdout: Some(stdout),
        stderr: None,
        outputs: BTreeMap::new(),
    };

    cache.put_action_result(action, &result).await?;
    let cached = cache.get_action_result(action).await?;

    assert_eq!(cached, Some(result));
    Ok(())
}
```

| API | Use |
| --- | --- |
| `put_action_result(action, result)` | Stores a cached result for an action digest. |
| `get_action_result(action)` | Returns a cached result when one exists. |
| `forget_action(action)` | Removes one cached action result. Referenced blobs are left intact. |
| `stats()` | Returns local cache statistics. |

## Types

| Type | Use |
| --- | --- |
| `Digest` | Identifies blobs and action cache entries. |
| `ActionResult` | Stores an action exit code, stdout digest, stderr digest, and output digests. |
| `Stats` | Reports local cache size and entry counts. |
| `CacheProvider` | Provides lower-level cache access for integrations that already use the provider layer. |
| `Result<T>` | SDK result alias. |
| `Error` | Reports invalid digest strings and cache provider errors. |

Use `digest_from_hex(hex)` when a digest crosses a string boundary, such
as JSON, configuration, or another process. It validates lowercase BLAKE3
hex strings and returns `Error::InvalidDigest` for invalid input.
