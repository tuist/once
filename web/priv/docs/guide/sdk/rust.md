---
prev: false
next: false
---

# Rust Software Development Kit

The Rust library is the `struo` crate. It exposes cache primitives for
embedding Struo in Rust applications and tools. Script execution belongs to
the command line and is not part of this library. See the
[language-library overview](/guide/sdk/) to compare the available bindings.

```rust
#[tokio::main]
async fn main() -> struo::Result<()> {
    let cache = struo::Cache::new();
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
struo = { git = "https://github.com/tuist/struo" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

The crate is released from this repository for source integration. It is
not currently published to the crates.io registry.

## Cache

`struo::Cache` is the high-level cache client. It is cheap to clone and can
be reused across blob and action-result operations.

### Constructors

Choose a constructor based on who owns provider selection. The workspace
constructor uses the same effective configuration as the command line.

| Application programming interface | Use |
| --- | --- |
| `Cache::new()` | Opens the default local cache using the operating-system convention. |
| `Cache::local(root)` | Opens an isolated local cache at a caller-owned root. |
| `Cache::from_workspace(root)` | Resolves the same effective provider as the command line for this workspace, including Tuist, object storage, and HTTP cache providers. |
| `Cache::with_provider(cache)` | Wraps an existing `CacheProvider`. |

The default follows the
[X Desktop Group base-directory convention](https://specifications.freedesktop.org/basedir-spec/latest/):
its cache root is `$XDG_CACHE_HOME/struo/cas` when
`XDG_CACHE_HOME` is set, and `$HOME/.cache/struo/cas` otherwise.

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

### Read-Only Blob Staging

Use `cache.provider().stage_blob(&digest, parent).await` to prepare a blob for
read-only, seekable access without loading the whole payload into memory.
The parent directory must already exist and is owned by the caller. The
returned `StagedBlob` owns its temporary storage; `into_reader()` transfers
that ownership to a `StagedBlobReader` implementing synchronous `Read` and
`Seek`. Neither type exposes a mutable file handle or storage path. Dropping
the lease or its reader removes temporary storage.

Raw immutable blobs can be pinned with a read-only file handle without creating
temporary files or requiring hardlink support. Open pins have a process-wide
budget: excess leases use bounded copying rather than waiting for earlier
leases to close. Compressed blobs also use private temporary storage and open
their reader only when requested. All staged blobs are verified before a reader
is returned, including pinned raw files. Ephemeral decoded staging does not
require a durability sync; persistent cache writes and mutable file copies
retain their durability checks. Corrupt local content is removed; a remote
provider refetches it once before returning a staging failure.
A successful lease remains readable if the cache subsequently removes the
original blob. Ordinary `get_blob_to_file` and `copy_blob_to_file` destinations
remain independent, mutable copies.

`stage_blob_guarded(&digest, parent, guard)` additionally retains a
`Send + 'static` owner through remote ingestion and local blocking decoding.
If staging is cancelled, active workers keep their owner until they finish;
a local decoder also retains and then removes its private temporary directory.
The completed lease does not retain the guard. A cancelled operation can drop
the guard on a blocking worker thread after the calling future has been dropped.
The existing cache selection, remote retrieval and publication defaults are
unchanged. These APIs do not execute actions or create runtime sessions.

When a caller already knows a collection of required digests,
`cache.provider().prefetch_blobs(&digests).await` fetches missing remote content
into the local tier before staging. It submits bounded, deduplicated windows
and packs small blobs into size-limited requests; large blobs still stream.
A local-only provider does nothing. Prefetch does not pin content or verify
existing local copies, so reads and staging still verify and recover from
eviction or corruption. `prefetch_blobs_guarded(&digests, guard)` retains a
`Send + 'static` owner through active ingestion if the caller is cancelled.
All transfers share the provider's existing memory, staging and concurrency
budgets, and persistent CAS writes retain their durability guarantees.

### Owned Blob Producers

The lower-level provider also offers `put_produced(guard, producer)` for
synchronous serializers. The owned producer runs on a blocking worker and
writes sequential bytes through `std::io::Write`, avoiding an intermediate
serialized file. Both arguments must be `Send + 'static`. Use `()` as the
guard when no additional lifetime reservation is needed.

```rust
let digest = cache.provider().put_produced((), |writer| {
    writer.write_all(b"generated content")
}).await?;
```

The guard stays with the worker through encoding and durable local publication.
Dropping the future removes its scratch paths immediately; subsequent writes
fail, but a producer already doing other blocking work keeps its guard until
that work ends. Producers must bound their own buffers. No blob is published
unless the producer and final encoding succeed. Remote mirroring follows the
same size-bounded policy as `put_stream`; existing cache defaults are unchanged.

Integrations that already own capacity for two workers can opt into
`put_produced_pipelined(guard, producer)`. Its guard must cover both workers.
Digest calculation overlaps encoding through a one-chunk queue with chunks
capped at 512 KiB. The producer joins the digest worker on success, errors and
cancellation before releasing the guard. When extra capacity is unavailable,
use the serial `put_produced` method instead of waiting while holding a partial
reservation. Blob bytes, digest validation and durable publication are unchanged.

### Action Keys

`ActionKeyBuilder` gives cached automation a versioned, unambiguous identity.
Start with a namespace owned by the integration, then add labeled values and
content digests in deterministic order.

```rust
let source = cache.put_file("inputs/source").await?;
let mut key = struo::ActionKeyBuilder::new("example.compile");
key.push_bytes("tool", "compiler")
    .push_bytes("configuration", "release")
    .push_digest("source", source);
let action = key.finish();
```

Changing the namespace, a label, an input value, a digest, or input order
produces a different action key. Inputs are hashed as they are added, without
retaining a separate copy of their contents. Builder memory stays
bounded independently of input size, and cloning a builder copies only its
hash state. The versioned encoding and resulting action keys are unchanged.

### Action Results

Action results let embedders associate an action digest with an exit code,
stdout digest, stderr digest, and output digests.

```rust
use std::collections::BTreeMap;

#[tokio::main]
async fn main() -> struo::Result<()> {
    let cache = struo::Cache::new();
    let stdout = cache.put_blob(b"compiled").await?;
    let mut key = struo::ActionKeyBuilder::new("example.compile");
    key.push_bytes("tool", "example");
    let action = key.finish();
    let result = struo::ActionResult {
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
level Struo modules.

| Type | Use |
| --- | --- |
| `Digest` | Identifies blobs and action cache entries. |
| `ActionResult` | Stores an action exit code, stdout digest, stderr digest, and output digests. |
| `Stats` | Reports local cache size and entry counts. |
| `StagedBlob` | Owns temporary storage for read-only blob access. |
| `StagedBlobReader` | Reads and seeks within a staged blob while retaining its storage lease. |
| `CacheProvider` | Provides lower-level cache access for integrations that already own a provider. |
| `TuistCacheConfig` | Configures a manually constructed Tuist cache provider. |
| `ActionKeyBuilder` | Builds a versioned action digest from ordered, labeled inputs. |
| `Result<T>` | Library result alias. |
| `Error` | Reports invalid digest strings and cache provider errors. |

Use `digest_from_hex(hex)` when a digest crosses a string boundary, such
as structured data, configuration, or another process. It validates lowercase
BLAKE3 hexadecimal strings and returns `Error::InvalidDigest` for invalid
input.

`Error::Cache` wraps a cache failure that can classify itself. Call
`is_not_found()` to tell confirmed absence apart from `is_unavailable()`, a
remote service that could not answer, and `is_corrupt()`, content that failed
its integrity check. `kind()` returns the full classification, which also
distinguishes refused credentials, malformed remote answers, configuration
problems, transfers larger than their budget, and local filesystem failures.
Only confirmed absence means the content is not cached; see
[Misses, Failures, And Integrity](/guide/sdk/#misses-failures-and-integrity).

### Matching On Cache Errors

The classification methods are the compatible way to react to a cache
failure. The cache error type gained a variant for classified remote
failures in this release, and that error type is not marked as
non-exhaustive, so code that matches every variant of it without a wildcard
arm stops compiling and needs a new arm. Failures that earlier releases
reported as a generic remote error, or as a miss, now arrive in that new
variant. Match on `kind()` or the `is_*` methods instead of on variants, and
always keep a wildcard arm when matching on the classification itself,
because new kinds can be added without a breaking release. Display text for
remote failures is unchanged.

```rust
match cache.get_action_result(action).await {
    Ok(Some(result)) => replay(result),
    Ok(None) => run_and_store().await?,
    Err(struo::Error::Cache(error)) if error.is_unavailable() => run_without_cache().await?,
    Err(error) => return Err(error.into()),
}
```
