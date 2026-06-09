export interface ActionResult {
  exitCode: number;
  stdout?: string | null;
  stderr?: string | null;
  outputs?: Record<string, string>;
}

export interface CacheStats {
  blobCount: number;
  blobBytes: number;
  actionCount: number;
  actionBytes: number;
}

export class OnceError extends Error {}

export class Cache {
  version(): string;
  /**
   * Strings are encoded as UTF-8 before hashing.
   */
  digest(bytes: Buffer | Uint8Array | string): string;
  /**
   * Strings are encoded as UTF-8 before being stored.
   *
   * Blob bytes currently cross the native boundary as JSON arrays, so very
   * large blobs should use the Rust SDK or CLI until the JavaScript SDK has a
   * streaming native ABI.
   */
  putBlob(bytes: Buffer | Uint8Array | string): Promise<string>;
  getBlob(digest: string): Promise<Buffer>;
  hasBlob(digest: string): Promise<boolean>;
  putActionResult(result: ActionResult, actionDigest: string): Promise<boolean>;
  getActionResult(actionDigest: string): Promise<ActionResult | null>;
  forgetAction(actionDigest: string): Promise<boolean>;
  stats(): Promise<CacheStats>;
}

/**
 * Strings are encoded as UTF-8 before hashing.
 */
export function digest(bytes: Buffer | Uint8Array | string): string;
