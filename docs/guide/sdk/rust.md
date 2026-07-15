---
prev: false
next: false
---

# Rust Software Development Kit

The Rust library is the `once` crate. It exposes cache primitives for
embedding Once in Rust applications and tools. Script execution belongs to
the command line and is not part of this library. See the
[language-library overview](/guide/sdk/) to compare the available bindings.

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

Choose a constructor based on who owns the cache location. Most
applications should use the default based on the
[X Desktop Group base-directory convention](https://specifications.freedesktop.org/basedir-spec/latest/)
and only pass a path when they need isolation or deterministic test setup.

| Application programming interface | Use |
| --- | --- |
| `Cache::new()` | Opens the default local cache using the operating-system convention. |
| `Cache::with_provider(cache)` | Wraps an existing `CacheProvider`. |

The default cache root is `$XDG_CACHE_HOME/once/cas` when
`XDG_CACHE_HOME` is set, and `$HOME/.cache/once/cas` otherwise.

### Introspection

Use these methods when your integration needs to inspect how the cache was
configured, for example to display diagnostics or pass the lower-level
provider into code that already works with content-addressed storage
primitives.

| Application programming interface | Use |
| --- | --- |
| `provider()` | Returns the underlying `CacheProvider`. |
| `root()` | Returns the local root directory used by the provider. |

### Blobs

Blobs are content-addressed byte payloads. Store bytes once, then refer to
them by digest from action results, manifests, or other integration state.

| Application programming interface | Use |
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

The action-result methods store and retrieve metadata for a completed action.
They do not run commands. The caller is responsible for defining the
action payload, hashing it, executing any work, and deciding which blob
digests belong in the cached result.

| Application programming interface | Use |
| --- | --- |
| `put_action_result(action, result)` | Stores a cached result for an action digest. |
| `get_action_result(action)` | Returns a cached result when one exists. |
| `forget_action(action)` | Removes one cached action result. Referenced blobs are left intact. |
| `stats()` | Returns local cache statistics. |

## Types

These are the public supporting types exposed by the crate root. They are
re-exported so embedders can model cache state without importing lower
level Once modules.

| Type | Use |
| --- | --- |
| `Digest` | Identifies blobs and action cache entries. |
| `ActionResult` | Stores an action exit code, stdout digest, stderr digest, and output digests. |
| `Stats` | Reports local cache size and entry counts. |
| `CacheProvider` | Provides lower-level cache access for integrations that already own a provider. |
| `Result<T>` | Library result alias. |
| `Error` | Reports invalid digest strings and cache provider errors. |

Use `digest_from_hex(hex)` when a digest crosses a string boundary, such
as structured data, configuration, or another process. It validates lowercase
BLAKE3 hexadecimal strings and returns `Error::InvalidDigest` for invalid
input.
