---
prev: false
next: false
---

# Ruby Software Development Kit

The Ruby library is the `buildonce` gem. It exposes Once cache primitives for
Ruby applications and tools. Script execution belongs to the command line and
is not part of this library. See the [language-library overview](/guide/sdk/)
to compare the available bindings.

```ruby
require "buildonce"

cache = Once::Cache.new
digest = cache.put_blob("hello")
bytes = cache.get_blob(digest)

raise unless bytes == "hello"
```

## Installation

Install the gem from RubyGems:

```sh
gem install buildonce
```

The gem includes prebuilt native Once libraries for supported platforms.
Set `ONCE_LIBRARY_PATH` when you need to load a custom `libonce` build.

## Cache

`Once::Cache` can open the default local cache, an explicit local root, or the
effective provider for a workspace. The default follows the
[X Desktop Group base-directory convention](https://specifications.freedesktop.org/basedir-spec/latest/):
its cache root is `$XDG_CACHE_HOME/once/cas` when `XDG_CACHE_HOME` is
set, and `$HOME/.cache/once/cas` otherwise.

Ruby methods are synchronous. `digest(bytes)` only hashes bytes and
does not touch storage. When `bytes` is a string, the library uses the
string's byte representation.

| Application programming interface | Use |
| --- | --- |
| `Once::Cache.new` | Opens the default local cache using the operating-system convention. |
| `Once::Cache.new(local_cache_root:)` | Opens an isolated local cache at a caller-owned root. |
| `Once::Cache.new(workspace_root:)` | Resolves the effective provider for a workspace. |
| `version` | Returns the linked Once version. |
| `digest(bytes)` | Returns the content digest for bytes without writing them to the cache. |
| `put_blob(bytes)` | Stores bytes and returns their content digest. |
| `put_file(path)` | Stores a file without loading its complete contents into Ruby memory. |
| `get_blob(digest)` | Reads bytes for a digest. |
| `get_blob_to_file(digest, path)` | Writes a blob to a file and returns its byte count. |
| `has_blob?(digest)` | Returns whether a blob exists. |
| `put_action_result(result, action_digest:)` | Stores a cached result for an action digest. |
| `get_action_result(action_digest)` | Returns a cached result when one exists. |
| `forget_action(action_digest)` | Removes one cached action result. Referenced blobs are left intact. |
| `stats` | Returns local cache statistics. |

Prefer `put_file` and `get_blob_to_file` for logs, archives, compiler outputs,
and other payloads whose size is not tightly bounded.

## Action Keys

`Once::ActionKey` builds a versioned identity from ordered, labeled inputs:

```ruby
source = cache.put_file("inputs/source")
action_digest = Once::ActionKey.new("example.compile")
                               .add_bytes("tool", "compiler")
                               .add_digest("source", source)
                               .digest
```

Input order is significant and must be deterministic.

## Types

Use `Once::ActionResult` for cached action metadata:

```ruby
Once::ActionResult.new(
  exit_code: 0,
  stdout: stdout_digest,
  stderr: nil,
  outputs: {},
)
```

`stats` returns `Once::CacheStats` with `blob_count`, `blob_bytes`,
`action_count`, and `action_bytes`.
