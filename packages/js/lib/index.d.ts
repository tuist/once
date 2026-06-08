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
  digest(bytes: Buffer | Uint8Array | string): string;
  putBlob(bytes: Buffer | Uint8Array | string): Promise<string>;
  getBlob(digest: string): Promise<Buffer>;
  hasBlob(digest: string): Promise<boolean>;
  putActionResult(result: ActionResult, actionDigest: string): Promise<boolean>;
  getActionResult(actionDigest: string): Promise<ActionResult | null>;
  forgetAction(actionDigest: string): Promise<boolean>;
  stats(): Promise<CacheStats>;
}

export function digest(bytes: Buffer | Uint8Array | string): string;
