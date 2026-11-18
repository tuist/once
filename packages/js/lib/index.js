"use strict";

const fs = require("node:fs");
const path = require("node:path");
const koffi = require("koffi");

class StruoError extends Error {
  constructor(message) {
    super(message);
    this.name = "StruoError";
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
    return native.struo_version() || "";
  }

  digest(bytes) {
    const buffer = toBuffer(bytes, "Cache#digest bytes");
    return decodeStringResponse(native.struo_digest_bytes(buffer, buffer.length));
  }

  async putBlob(bytes) {
    const buffer = toBuffer(bytes, "Cache#putBlob bytes");
    return decodeRequest(
      native.struo_cache_put_blob_json,
      { ...this.selection, bytes: Array.from(buffer) },
    );
  }

  async putFile(filePath) {
    validatePath(filePath, "filePath");
    return decodeRequest(native.struo_cache_put_file_json, {
      ...this.selection,
      path: filePath,
    });
  }

  async getBlob(digest) {
    validateDigest(digest, "digest");
    const response = decodeRequest(native.struo_cache_get_blob_json, {
      ...this.selection,
      digest,
    });
    return Buffer.from(response.bytes);
  }

  async getBlobToFile(digest, filePath) {
    validateDigest(digest, "digest");
    validatePath(filePath, "filePath");
    return decodeRequest(native.struo_cache_get_blob_to_file_json, {
      ...this.selection,
      digest,
      path: filePath,
    });
  }

  async hasBlob(digest) {
    validateDigest(digest, "digest");
    return decodeRequest(native.struo_cache_has_blob_json, {
      ...this.selection,
      digest,
    });
  }

  async putActionResult(result, actionDigest) {
    validateDigest(actionDigest, "actionDigest");
    return decodeRequest(
      native.struo_cache_put_action_result_json,
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
      native.struo_cache_get_action_result_json,
      { ...this.selection, action_digest: actionDigest },
    );
    return result === null ? null : actionResultFromNative(result);
  }

  async forgetAction(actionDigest) {
    validateDigest(actionDigest, "actionDigest");
    return decodeRequest(
      native.struo_cache_forget_action_json,
      { ...this.selection, action_digest: actionDigest },
    );
  }

  async stats() {
    const stats = decodeRequest(native.struo_cache_stats_json, this.selection);
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
    return decodeRequest(native.struo_action_key_json, {
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
    throw new StruoError("native Struo function returned null");
  }
  let response;
  try {
    response = JSON.parse(pointer);
  } catch (error) {
    throw new StruoError(
      `native Struo response must be valid JavaScript Object Notation: ${error.message}`,
    );
  }
  if (response.status === "ok") {
    return response.value;
  }
  throw new StruoError(response.message || "Struo native call failed");
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
    throw new StruoError(`${name} must be a lowercase BLAKE3 hex digest`);
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
  const struoStringFree = library.func("void struo_string_free(void *value)");
  const struoString = koffi.disposable("StruoString", "str", struoStringFree);

  return {
    struo_version: library.func("struo_version", struoString, []),
    struo_digest_bytes: library.func("struo_digest_bytes", struoString, [
      "const void *",
      "size_t",
    ]),
    struo_action_key_json: library.func("struo_action_key_json", struoString, [
      "str",
    ]),
    struo_cache_put_blob_json: library.func(
      "struo_cache_put_blob_json",
      struoString,
      ["str"],
    ),
    struo_cache_put_file_json: library.func(
      "struo_cache_put_file_json",
      struoString,
      ["str"],
    ),
    struo_cache_get_blob_json: library.func(
      "struo_cache_get_blob_json",
      struoString,
      ["str"],
    ),
    struo_cache_get_blob_to_file_json: library.func(
      "struo_cache_get_blob_to_file_json",
      struoString,
      ["str"],
    ),
    struo_cache_has_blob_json: library.func(
      "struo_cache_has_blob_json",
      struoString,
      ["str"],
    ),
    struo_cache_put_action_result_json: library.func(
      "struo_cache_put_action_result_json",
      struoString,
      ["str"],
    ),
    struo_cache_get_action_result_json: library.func(
      "struo_cache_get_action_result_json",
      struoString,
      ["str"],
    ),
    struo_cache_forget_action_json: library.func(
      "struo_cache_forget_action_json",
      struoString,
      ["str"],
    ),
    struo_cache_stats_json: library.func("struo_cache_stats_json", struoString, [
      "str",
    ]),
  };
}

function resolveLibraryPath() {
  if (process.env.STRUO_LIBRARY_PATH) {
    return process.env.STRUO_LIBRARY_PATH;
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

  throw new StruoError(
    `missing native Struo library for ${process.platform}-${process.arch}; ` +
      "set STRUO_LIBRARY_PATH or install a package that includes this platform",
  );
}

function libraryName() {
  switch (process.platform) {
    case "darwin":
      return "libstruo.dylib";
    case "win32":
      return "struo.dll";
    default:
      return "libstruo.so";
  }
}

module.exports = {
  ActionKey,
  Cache,
  StruoError,
  digest,
};
