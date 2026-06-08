# JavaScript SDK

The JavaScript SDK is the `buildonce` npm package. It exposes cache
primitives for Node.js applications and tools. Script execution is CLI
specific and is not part of the SDK surface.

```js
const { Cache } = require("buildonce");

async function main() {
  const cache = new Cache();
  const digest = await cache.putBlob(Buffer.from("hello"));
  const bytes = await cache.getBlob(digest);

  console.assert(bytes.equals(Buffer.from("hello")));
}
```

## Installation

Install the package from npm:

```sh
npm install buildonce
```

The package includes prebuilt native Once libraries for supported
platforms. Set `ONCE_LIBRARY_PATH` when you need to load a custom
`libonce` build.

## Cache

`Cache` opens the default local cache using XDG conventions. The default
cache root is `$XDG_CACHE_HOME/once/cas` when `XDG_CACHE_HOME` is set, and
`$HOME/.cache/once/cas` otherwise.

| API | Use |
| --- | --- |
| `new Cache()` | Opens the default local cache using XDG conventions. |
| `version()` | Returns the linked Once version. |
| `digest(bytes)` | Returns the content digest for bytes without writing them to the cache. |
| `putBlob(bytes)` | Stores bytes and returns their content digest. |
| `getBlob(digest)` | Reads bytes for a digest. |
| `hasBlob(digest)` | Returns whether a blob exists. |
| `putActionResult(result, actionDigest)` | Stores a cached result for an action digest. |
| `getActionResult(actionDigest)` | Returns a cached result when one exists. |
| `forgetAction(actionDigest)` | Removes one cached action result. Referenced blobs are left intact. |
| `stats()` | Returns local cache statistics. |

## Types

Action results use JavaScript-friendly camel case:

```js
{
  exitCode: 0,
  stdout: "<stdout digest>",
  stderr: null,
  outputs: {}
}
```

`stats()` returns `blobCount`, `blobBytes`, `actionCount`, and
`actionBytes`.
