"use strict";

const fs = require("node:fs");
const path = require("node:path");
const koffi = require("koffi");

class OnceError extends Error {
  constructor(message) {
    super(message);
    this.name = "OnceError";
  }
}

const native = loadNative();
const DIGEST_PATTERN = /^[0-9a-f]{64}$/;

class Cache {
  version() {
    return native.once_version() || "";
  }

  digest(bytes) {
    const buffer = toBuffer(bytes, "Cache#digest bytes");
    return decodeStringResponse(native.once_digest_bytes(buffer, buffer.length));
  }

  async putBlob(bytes) {
    const buffer = toBuffer(bytes, "Cache#putBlob bytes");
    return decodeRequest(
      native.once_cache_put_blob_json,
      { bytes: Array.from(buffer) },
    );
  }

  async getBlob(digest) {
    validateDigest(digest, "digest");
    const response = decodeRequest(native.once_cache_get_blob_json, { digest });
    return Buffer.from(response.bytes);
  }

  async hasBlob(digest) {
    validateDigest(digest, "digest");
    return decodeRequest(native.once_cache_has_blob_json, { digest });
  }

  async putActionResult(result, actionDigest) {
    validateDigest(actionDigest, "actionDigest");
    return decodeRequest(
      native.once_cache_put_action_result_json,
      {
        action_digest: actionDigest,
        result: actionResultToNative(result),
      },
    );
  }

  async getActionResult(actionDigest) {
    validateDigest(actionDigest, "actionDigest");
    const result = decodeRequest(
      native.once_cache_get_action_result_json,
      { action_digest: actionDigest },
    );
    return result === null ? null : actionResultFromNative(result);
  }

  async forgetAction(actionDigest) {
    validateDigest(actionDigest, "actionDigest");
    return decodeRequest(
      native.once_cache_forget_action_json,
      { action_digest: actionDigest },
    );
  }

  async stats() {
    const stats = decodeRequest(native.once_cache_stats_json, {});
    return {
      blobCount: stats.blob_count,
      blobBytes: stats.blob_bytes,
      actionCount: stats.action_count,
      actionBytes: stats.action_bytes,
    };
  }
}

function digest(bytes) {
  return new Cache().digest(bytes);
}

function decodeStringResponse(pointer) {
  return decodeResponse(pointer);
}

function decodeRequest(fn, request) {
  const pointer = fn(JSON.stringify(request));
  return decodeResponse(pointer);
}

function decodeResponse(pointer) {
  if (pointer == null) {
    throw new OnceError("native Once function returned null");
  }
  let response;
  try {
    response = JSON.parse(pointer);
  } catch (error) {
    throw new OnceError(`native Once response must be valid JSON: ${error.message}`);
  }
  if (response.status === "ok") {
    return response.value;
  }
  throw new OnceError(response.message || "Once native call failed");
}

function actionResultToNative(result) {
  return {
    exit_code: result.exitCode,
    stdout: result.stdout ?? null,
    stderr: result.stderr ?? null,
    outputs: result.outputs ?? {},
  };
}

function actionResultFromNative(result) {
  return {
    exitCode: result.exit_code,
    stdout: result.stdout ?? null,
    stderr: result.stderr ?? null,
    outputs: result.outputs ?? {},
  };
}

function validateDigest(value, name) {
  if (typeof value !== "string" || !DIGEST_PATTERN.test(value)) {
    throw new OnceError(`${name} must be a lowercase BLAKE3 hex digest`);
  }
}

function toBuffer(bytes, name) {
  if (Buffer.isBuffer(bytes)) {
    return bytes;
  }
  if (bytes instanceof Uint8Array) {
    return Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }
  if (typeof bytes === "string") {
    return Buffer.from(bytes, "utf8");
  }
  throw new TypeError(`${name} must be a Buffer, Uint8Array, or string`);
}

function loadNative() {
  const library = koffi.load(resolveLibraryPath());
  const onceStringFree = library.func("void once_string_free(void *value)");
  const onceString = koffi.disposable("OnceString", "str", onceStringFree);

  return {
    once_version: library.func("once_version", onceString, []),
    once_digest_bytes: library.func("once_digest_bytes", onceString, [
      "const void *",
      "size_t",
    ]),
    once_cache_put_blob_json: library.func(
      "once_cache_put_blob_json",
      onceString,
      ["str"],
    ),
    once_cache_get_blob_json: library.func(
      "once_cache_get_blob_json",
      onceString,
      ["str"],
    ),
    once_cache_has_blob_json: library.func(
      "once_cache_has_blob_json",
      onceString,
      ["str"],
    ),
    once_cache_put_action_result_json: library.func(
      "once_cache_put_action_result_json",
      onceString,
      ["str"],
    ),
    once_cache_get_action_result_json: library.func(
      "once_cache_get_action_result_json",
      onceString,
      ["str"],
    ),
    once_cache_forget_action_json: library.func(
      "once_cache_forget_action_json",
      onceString,
      ["str"],
    ),
    once_cache_stats_json: library.func("once_cache_stats_json", onceString, [
      "str",
    ]),
  };
}

function resolveLibraryPath() {
  if (process.env.ONCE_LIBRARY_PATH) {
    return process.env.ONCE_LIBRARY_PATH;
  }

  const candidate = path.join(
    __dirname,
    "..",
    "prebuilds",
    `${process.platform}-${process.arch}`,
    libraryName(),
  );
  if (fs.existsSync(candidate)) {
    return candidate;
  }

  throw new OnceError(
    `missing native Once library for ${process.platform}-${process.arch}; ` +
      "set ONCE_LIBRARY_PATH or install a package that includes this platform",
  );
}

function libraryName() {
  switch (process.platform) {
    case "darwin":
      return "libonce.dylib";
    case "win32":
      return "once.dll";
    default:
      return "libonce.so";
  }
}

module.exports = {
  Cache,
  OnceError,
  digest,
};
