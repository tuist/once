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

Choose a constructor based on who owns provider selection. The workspace
constructor uses the same effective configuration as the command line.

| Application programming interface | Use |
| --- | --- |
| `Cache::new()` | Opens the default local cache using the operating-system convention. |
| `Cache::local(root)` | Opens an isolated local cache at a caller-owned root. |
| `Cache::from_workspace(root)` | Resolves the same effective provider as the command line for this workspace. |
| `Cache::with_provider(cache)` | Wraps an existing `CacheProvider`. |

The default follows the
[X Desktop Group base-directory convention](https://specifications.freedesktop.org/basedir-spec/latest/):
its cache root is `$XDG_CACHE_HOME/once/cas` when
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
| `put_stream(reader)` | Stores an asynchronous reader with bounded memory use. |
| `put_file(path)` | Stores a file without loading it completely into memory. |
| `get_blob(digest)` | Reads bytes for a digest. |
| `get_blob_to_file(digest, path)` | Writes a blob to a file and returns its byte count. |
| `has_blob(digest)` | Returns whether a blob exists. |

Prefer the file and stream methods for logs, archives, compiler outputs, and
other payloads whose size is not tightly bounded.

### Action Keys

`ActionKeyBuilder` gives cached automation a versioned, unambiguous identity.
Start with a namespace owned by the integration, then add labeled values and
content digests in deterministic order.

```rust
let source = cache.put_file("Sources/App.swift").await?;
let mut key = once::ActionKeyBuilder::new("example.compile");
key.push_bytes("compiler", "swiftc")
    .push_bytes("configuration", "debug")
    .push_digest("source", source);
let action = key.finish();
```

Changing the namespace, a label, an input value, a digest, or input order
produces a different action key.

### Action Results

Action results let embedders associate an action digest with an exit code,
stdout digest, stderr digest, and output digests.

```rust
use std::collections::BTreeMap;

#[tokio::main]
async fn main() -> once::Result<()> {
    let cache = once::Cache::new();
    let stdout = cache.put_blob(b"compiled").await?;
    let mut key = once::ActionKeyBuilder::new("example.compile");
    key.push_bytes("tool", "example");
    let action = key.finish();
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
They do not run commands. The caller is responsible for executing work,
including every relevant input in the action key, and deciding which blob
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
| `TuistCacheConfig` | Configures a manually constructed Tuist cache provider. |
| `ActionKeyBuilder` | Builds a versioned action digest from ordered, labeled inputs. |
| `Result<T>` | Library result alias. |
| `Error` | Reports invalid digest strings and cache provider errors. |

Use `digest_from_hex(hex)` when a digest crosses a string boundary, such
as structured data, configuration, or another process. It validates lowercase
BLAKE3 hexadecimal strings and returns `Error::InvalidDigest` for invalid
input.
