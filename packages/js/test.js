"use strict";

const assert = require("node:assert/strict");
const os = require("node:os");
const fs = require("node:fs");
const { Cache, digest } = require("./lib");

async function main() {
  const tmp = fs.mkdtempSync(`${os.tmpdir()}/once-js-`);
  process.env.XDG_CACHE_HOME = tmp;

  const cache = new Cache();
  const blobDigest = await cache.putBlob("hello");

  assert.equal(digest("hello"), blobDigest);
  assert.equal(await cache.hasBlob(blobDigest), true);
  assert.deepEqual(await cache.getBlob(blobDigest), Buffer.from("hello"));

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

  const stats = await cache.stats();
  assert.equal(typeof stats.blobCount, "number");
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
