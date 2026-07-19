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
  constructor(options = {}) {
    if (options == null || typeof options !== "object" || Array.isArray(options)) {
      throw new TypeError("Cache options must be an object");
    }
    const { localCacheRoot, workspaceRoot } = options;
    if (localCacheRoot != null && workspaceRoot != null) {
      throw new TypeError("localCacheRoot and workspaceRoot cannot be used together");
    }
    if (localCacheRoot != null && typeof localCacheRoot !== "string") {
      throw new TypeError("localCacheRoot must be a string");
    }
    if (workspaceRoot != null && typeof workspaceRoot !== "string") {
      throw new TypeError("workspaceRoot must be a string");
    }
    this.selection = {
      ...(localCacheRoot == null ? {} : { local_cache_root: localCacheRoot }),
      ...(workspaceRoot == null ? {} : { workspace_root: workspaceRoot }),
    };
  }

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
      { ...this.selection, bytes: Array.from(buffer) },
    );
  }

  async putFile(filePath) {
    validatePath(filePath, "filePath");
    return decodeRequest(native.once_cache_put_file_json, {
      ...this.selection,
      path: filePath,
    });
  }

  async getBlob(digest) {
    validateDigest(digest, "digest");
    const response = decodeRequest(native.once_cache_get_blob_json, {
      ...this.selection,
      digest,
    });
    return Buffer.from(response.bytes);
  }

  async getBlobToFile(digest, filePath) {
    validateDigest(digest, "digest");
    validatePath(filePath, "filePath");
    return decodeRequest(native.once_cache_get_blob_to_file_json, {
      ...this.selection,
      digest,
      path: filePath,
    });
  }

  async hasBlob(digest) {
    validateDigest(digest, "digest");
    return decodeRequest(native.once_cache_has_blob_json, {
      ...this.selection,
      digest,
    });
  }

  async putActionResult(result, actionDigest) {
    validateDigest(actionDigest, "actionDigest");
    return decodeRequest(
      native.once_cache_put_action_result_json,
      {
        ...this.selection,
        action_digest: actionDigest,
        result: actionResultToNative(result),
      },
    );
  }

  async getActionResult(actionDigest) {
    validateDigest(actionDigest, "actionDigest");
    const result = decodeRequest(
      native.once_cache_get_action_result_json,
      { ...this.selection, action_digest: actionDigest },
    );
    return result === null ? null : actionResultFromNative(result);
  }

  async forgetAction(actionDigest) {
    validateDigest(actionDigest, "actionDigest");
    return decodeRequest(
      native.once_cache_forget_action_json,
      { ...this.selection, action_digest: actionDigest },
    );
  }

  async stats() {
    const stats = decodeRequest(native.once_cache_stats_json, this.selection);
    return {
      blobCount: stats.blob_count,
      blobBytes: stats.blob_bytes,
      actionCount: stats.action_count,
      actionBytes: stats.action_bytes,
    };
  }
}

class ActionKey {
  constructor(namespace) {
    if (typeof namespace !== "string") {
      throw new TypeError("ActionKey namespace must be a string");
    }
    this.namespace = namespace;
    this.inputs = [];
  }

  addBytes(label, bytes) {
    validateLabel(label);
    const buffer = toBuffer(bytes, "ActionKey#addBytes bytes");
    this.inputs.push({ kind: "bytes", label, bytes: Array.from(buffer) });
    return this;
  }

  addDigest(label, digest) {
    validateLabel(label);
    validateDigest(digest, "digest");
    this.inputs.push({ kind: "digest", label, digest });
    return this;
  }

  digest() {
    return decodeRequest(native.once_action_key_json, {
      namespace: this.namespace,
      inputs: this.inputs,
    });
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
    throw new OnceError(
      `native Once response must be valid JavaScript Object Notation: ${error.message}`,
    );
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

function validateLabel(value) {
  if (typeof value !== "string") {
    throw new TypeError("ActionKey input label must be a string");
  }
}

function validatePath(value, name) {
  if (typeof value !== "string" || value.length === 0) {
    throw new TypeError(`${name} must be a non-empty string`);
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
    once_action_key_json: library.func("once_action_key_json", onceString, [
      "str",
    ]),
    once_cache_put_blob_json: library.func(
      "once_cache_put_blob_json",
      onceString,
      ["str"],
    ),
    once_cache_put_file_json: library.func(
      "once_cache_put_file_json",
      onceString,
      ["str"],
    ),
    once_cache_get_blob_json: library.func(
      "once_cache_get_blob_json",
      onceString,
      ["str"],
    ),
    once_cache_get_blob_to_file_json: library.func(
      "once_cache_get_blob_to_file_json",
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
  ActionKey,
  Cache,
  OnceError,
  digest,
};
