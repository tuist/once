# Once Ruby SDK

`tuist-once` exposes Once primitives to Ruby. It loads the same
native Once library used by the other SDKs and does not expose script
execution.

```ruby
require "once"

cache = Once::Cache.new
digest = cache.put_blob("hello")
bytes = cache.get_blob(digest)

raise unless bytes == "hello"
```

The gem looks for a bundled native library under `prebuilds/`. Set
`ONCE_LIBRARY_PATH` to load a custom `libonce` build.
