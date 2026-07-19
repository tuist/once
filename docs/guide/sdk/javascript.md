---
prev: false
next: false
---

# JavaScript Software Development Kit

The JavaScript library is the `buildonce` npm package. It exposes cache
primitives for Node.js applications and tools. Script execution belongs to
the command line and is not part of this library. See the
[language-library overview](/guide/sdk/) to compare the available bindings.

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

`Cache` can open the default local cache, an explicit local root, or the
effective provider for a workspace. The default follows the
[X Desktop Group base-directory convention](https://specifications.freedesktop.org/basedir-spec/latest/):
its cache root is `$XDG_CACHE_HOME/once/cas` when `XDG_CACHE_HOME` is set, and
`$HOME/.cache/once/cas` otherwise.

`version()` and `digest(bytes)` are synchronous. Cache storage operations
return promises and must be awaited. When `bytes` is a string, the library
encodes it using [Unicode Transformation Format, 8-bit](https://www.unicode.org/faq/utf_bom.html#UTF8)
before hashing or storing it.

| Application programming interface | Use |
| --- | --- |
| `new Cache()` | Opens the default local cache using the operating-system convention. |
| `new Cache({ localCacheRoot })` | Opens an isolated local cache at a caller-owned root. |
| `new Cache({ workspaceRoot })` | Resolves the effective provider for a workspace. |
| `version()` | Synchronously returns the linked Once version. |
| `digest(bytes)` | Synchronously returns the content digest for bytes without writing them to the cache. |
| `putBlob(bytes)` | Stores bytes and returns a promise for their content digest. |
| `putFile(path)` | Stores a file without loading its complete contents into JavaScript memory. |
| `getBlob(digest)` | Reads bytes for a digest and returns a promise. |
| `getBlobToFile(digest, path)` | Writes a blob to a file and returns a promise for its byte count. |
| `hasBlob(digest)` | Returns a promise for whether a blob exists. |
| `putActionResult(result, actionDigest)` | Stores a cached result for an action digest and returns a promise. |
| `getActionResult(actionDigest)` | Returns a promise for a cached result when one exists. |
| `forgetAction(actionDigest)` | Removes one cached action result and returns a promise. Referenced blobs are left intact. |
| `stats()` | Returns a promise for local cache statistics. |

Prefer `putFile` and `getBlobToFile` for logs, archives, compiler outputs, and
other payloads whose size is not tightly bounded.

## Action Keys

`ActionKey` builds a synchronous, versioned identity from ordered, labeled
inputs:

```js
const { ActionKey } = require("buildonce");

const source = await cache.putFile("Sources/App.swift");
const actionDigest = new ActionKey("example.compile")
  .addBytes("compiler", "swiftc")
  .addDigest("source", source)
  .digest();
```

Input order is significant and must be deterministic.

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
