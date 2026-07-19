# Once Ruby Software Development Kit

`buildonce` exposes Once primitives to Ruby. It loads the same
native Once library used by the other language libraries and does not expose script
execution.

```ruby
require "buildonce"

cache = Once::Cache.new
digest = cache.put_blob("hello")
bytes = cache.get_blob(digest)

raise unless bytes == "hello"
```

The gem looks for a bundled native library under `prebuilds/`. Set
`ONCE_LIBRARY_PATH` to load a custom `libonce` build.

Use `Once::Cache.new(workspace_root:)` to share the effective provider
configured for a repository, or `local_cache_root:` for isolation. Use
`put_file` and `get_blob_to_file` for large payloads. `Once::ActionKey` builds
a stable digest from ordered, labeled inputs.
