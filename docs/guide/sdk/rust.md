# Rust SDK

The Rust SDK is the `once` crate. It exposes cache primitives for
embedding Once in Rust applications and tools. Script execution is CLI
specific and is not part of the SDK surface.

```rust
# async fn example() -> once::Result<()> {
let cache = once::Cache::new();
let digest = cache.put_blob(b"hello").await?;
let bytes = cache.get_blob(digest).await?;

assert_eq!(bytes, b"hello");
# Ok(())
# }
```

## Installation

Add the crate to your `Cargo.toml`:

```toml
[dependencies]
once = { git = "https://github.com/tuist/once" }
```

The crate is released from this repository for source integration. It is
not currently published to the crates.io registry.

## Cache

`once::Cache` is the high-level cache client. It is cheap to clone and can
be reused across blob and action-result operations.

### Constructors

`Cache::new()` opens the default local cache using XDG conventions:

- `$XDG_CACHE_HOME/once/cas` when `XDG_CACHE_HOME` is set
- `$HOME/.cache/once/cas` otherwise

`Cache::local(local_cache_root)` opens a local filesystem cache at an
explicit root. Use this for tests, isolated sandboxes, and applications
that need a caller-owned cache location.

`Cache::with_provider(cache)` wraps an existing `CacheProvider`.

### Introspection

`provider()` returns the underlying `CacheProvider`.

`root()` returns the local root directory used by the provider.

### Blobs

`put_blob(bytes)` stores bytes and returns their content digest.

`get_blob(digest)` reads bytes for a digest.

`has_blob(digest)` returns whether a blob exists.

### Action Results

Action results let embedders associate an action digest with an exit code,
stdout digest, stderr digest, and output digests.

```rust
use std::collections::BTreeMap;

# async fn example() -> once::Result<()> {
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
# Ok(())
# }
```

`put_action_result(action, result)` stores a cached result for an action
digest.

`get_action_result(action)` returns `Some(ActionResult)` when the action
has a cached result, or `None` when it does not.

`forget_action(action)` removes one cached action result and returns
whether a result was removed. Referenced blobs are left intact.

`stats()` returns local cache statistics.

## Types

`Digest` identifies blobs and action cache entries.

`digest_from_hex(hex)` parses a lowercase BLAKE3 hex digest and returns an
SDK error for invalid input.

`ActionResult` stores the cached result of an action.

`Stats` reports cache size and entry counts.

`CacheProvider` is the lower-level provider type used by `Cache`.

`Result<T>` is the SDK result alias.

`Error` currently reports invalid digest strings and cache provider
errors.
