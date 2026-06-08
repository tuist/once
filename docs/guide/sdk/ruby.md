# Ruby SDK

The Ruby SDK is the `tuist-once` gem. It exposes Once primitives for Ruby
applications and tools. Script execution is CLI specific and is not part
of the SDK surface.

```ruby
require "once"

cache = Once::Cache.new
digest = cache.put_blob("hello")
bytes = cache.get_blob(digest)

raise unless bytes == "hello"
```

## Installation

Install the gem from RubyGems:

```sh
gem install tuist-once
```

The gem includes prebuilt native Once libraries for supported platforms.
Set `ONCE_LIBRARY_PATH` when you need to load a custom `libonce` build.

## Cache

`Once::Cache` opens the default local cache using XDG conventions. The
default cache root is `$XDG_CACHE_HOME/once/cas` when `XDG_CACHE_HOME` is
set, and `$HOME/.cache/once/cas` otherwise.

| API | Use |
| --- | --- |
| `Once::Cache.new` | Opens the default local cache using XDG conventions. |
| `version` | Returns the linked Once version. |
| `digest(bytes)` | Returns the content digest for bytes without writing them to the cache. |
| `put_blob(bytes)` | Stores bytes and returns their content digest. |
| `get_blob(digest)` | Reads bytes for a digest. |
| `has_blob?(digest)` | Returns whether a blob exists. |
| `put_action_result(result, action_digest:)` | Stores a cached result for an action digest. |
| `get_action_result(action_digest)` | Returns a cached result when one exists. |
| `forget_action(action_digest)` | Removes one cached action result. Referenced blobs are left intact. |
| `stats` | Returns local cache statistics. |

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
