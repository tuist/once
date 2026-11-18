import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

function option(name) {
  const index = process.argv.indexOf(name);
  if (index === -1 || index + 1 >= process.argv.length) {
    throw new Error(`missing ${name}`);
  }
  return process.argv[index + 1];
}

const output = option("--output");
const byteCount = Number.parseInt(option("--bytes"), 10);
const delayMilliseconds = Number.parseInt(option("--delay-ms"), 10);
const separator = process.argv.indexOf("--");
const inputs = separator === -1 ? [] : process.argv.slice(separator + 1);

const hash = createHash("sha256");
for (const input of inputs.sort()) {
  hash.update(readFileSync(input));
  hash.update("\0");
}

const seed = hash.digest();
const bytes = Buffer.alloc(byteCount);
for (let offset = 0; offset < byteCount; offset += seed.length) {
  seed.copy(bytes, offset, 0, Math.min(seed.length, byteCount - offset));
}

if (delayMilliseconds > 0) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, delayMilliseconds);
}

mkdirSync(dirname(output), { recursive: true });
writeFileSync(output, bytes);
