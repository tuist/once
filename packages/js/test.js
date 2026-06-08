"use strict";

const assert = require("node:assert/strict");
const os = require("node:os");
const fs = require("node:fs");
const childProcess = require("node:child_process");
const { Cache, OnceError, digest } = require("./lib");

async function assertRejectsOnceError(work) {
  await assert.rejects(work, OnceError);
}

async function main() {
  const tmp = fs.mkdtempSync(`${os.tmpdir()}/once-js-`);
  process.env.XDG_CACHE_HOME = tmp;

  const cache = new Cache();
  const blobDigest = await cache.putBlob("hello");

  assert.equal(digest("hello"), blobDigest);
  assert.equal(digest(""), await cache.putBlob(""));
  assert.equal(digest("é"), digest(Buffer.from("é", "utf8")));
  assert.equal(await cache.hasBlob(blobDigest), true);
  assert.deepEqual(await cache.getBlob(blobDigest), Buffer.from("hello"));
  assert.deepEqual(await cache.getBlob(digest("")), Buffer.alloc(0));
  await assertRejectsOnceError(() => cache.getBlob("not-a-digest"));
  await assertRejectsOnceError(() => cache.hasBlob("not-a-digest"));

  const actionDigest = digest("action");
  const result = {
    exitCode: 0,
    stdout: blobDigest,
    stderr: null,
    outputs: {},
  };
  assert.equal(await cache.putActionResult(result, actionDigest), true);
  assert.deepEqual(await cache.getActionResult(actionDigest), result);
  assert.equal(await cache.forgetAction(actionDigest), true);
  assert.equal(await cache.getActionResult(actionDigest), null);
  await assertRejectsOnceError(() => cache.putActionResult(result, "not-a-digest"));
  await assertRejectsOnceError(() => cache.getActionResult("not-a-digest"));
  await assertRejectsOnceError(() => cache.forgetAction("not-a-digest"));

  const digests = await Promise.all([
    cache.putBlob("one"),
    cache.putBlob("two"),
    cache.putBlob("three"),
  ]);
  assert.deepEqual(
    await Promise.all(digests.map((item) => cache.hasBlob(item))),
    [true, true, true],
  );

  const stats = await cache.stats();
  assert.equal(typeof stats.blobCount, "number");

  const missingLibrary = childProcess.spawnSync(
    process.execPath,
    ["-e", "require('./lib')"],
    {
      cwd: __dirname,
      env: {
        ...process.env,
        ONCE_LIBRARY_PATH: "/missing/libonce.dylib",
      },
      encoding: "utf8",
    },
  );
  assert.notEqual(missingLibrary.status, 0);
  assert.match(missingLibrary.stderr, /missing|cannot open|no such file/i);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
