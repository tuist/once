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

export interface CacheOptions {
  localCacheRoot?: string;
  workspaceRoot?: string;
}

export class Cache {
  constructor(options?: CacheOptions);
  version(): string;
  /**
   * Strings use Unicode Transformation Format, 8-bit before hashing.
   */
  digest(bytes: Buffer | Uint8Array | string): string;
  /**
   * Strings use Unicode Transformation Format, 8-bit before being stored.
   *
   * Use putFile for large blobs to avoid loading the complete payload into
   * JavaScript memory.
   */
  putBlob(bytes: Buffer | Uint8Array | string): Promise<string>;
  putFile(path: string): Promise<string>;
  getBlob(digest: string): Promise<Buffer>;
  getBlobToFile(digest: string, path: string): Promise<number>;
  hasBlob(digest: string): Promise<boolean>;
  putActionResult(result: ActionResult, actionDigest: string): Promise<boolean>;
  getActionResult(actionDigest: string): Promise<ActionResult | null>;
  forgetAction(actionDigest: string): Promise<boolean>;
  stats(): Promise<CacheStats>;
}

export class ActionKey {
  constructor(namespace: string);
  addBytes(label: string, bytes: Buffer | Uint8Array | string): this;
  addDigest(label: string, digest: string): this;
  digest(): string;
}

/**
 * Strings use Unicode Transformation Format, 8-bit before hashing.
 */
export function digest(bytes: Buffer | Uint8Array | string): string;
