# Once JavaScript Software Development Kit

`buildonce` exposes Once primitives to Node.js. It loads the same
native Once library used by the other language libraries and does not expose script
execution.

```js
const { Cache } = require("buildonce");

async function example() {
  const cache = new Cache();
  const digest = await cache.putBlob(Buffer.from("hello"));
  const bytes = await cache.getBlob(digest);

  console.assert(bytes.equals(Buffer.from("hello")));
}
```

The package looks for a bundled native library under `prebuilds/`. Set
`ONCE_LIBRARY_PATH` to load a custom `libonce` build.

Use `new Cache({ workspaceRoot })` to share the effective provider configured
for a repository, or `new Cache({ localCacheRoot })` for isolation. Use
`putFile` and `getBlobToFile` for large payloads so the host does not need to
hold their complete contents in JavaScript memory. `ActionKey` builds a stable
digest from ordered, labeled inputs.
