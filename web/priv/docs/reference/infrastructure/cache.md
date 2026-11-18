# Cache Provider Contract

A cache service stores content and maps action keys to results. It does not
run builds or need worker machines, machine images, or an execution API.
Start with the [provider guide](/guide/infrastructure/providers) for the
integration steps.

The service behavior below applies to any cache integration. The
[REAPI transport](#existing-reapi-transport) describes compatibility with the
current Tuist adapter, not a mandatory protocol for a new adapter.

## Provider-Neutral Operations

Your service must support:

- **Store content:** accept bytes addressed by digest. Repeating or
  concurrently uploading the same content must be safe. Do not expose an
  incomplete upload as a stored blob or accept bytes that do not match the
  declared digest.
- **Read content:** return the exact stored bytes or confirmed absence.
- **Check availability:** report which requested blobs are missing. A
  failed or unauthorized request is not evidence that content is missing.
- **Store a result:** retain an action key, exit code, and references to
  output and captured-stream blobs. The adapter uploads referenced content
  before publishing the result.
- **Look up a result:** return the stored record or confirmed absence.
  Struo checks its references before reusing a remote result.

Action keys are opaque to the service. Struo decides whether an action can
reuse a result; the cache does not recompute keys or interpret build rules.

## Content and Action Results

Struo uses BLAKE3-256 digests, serialized in JSON as 64 lowercase hexadecimal
characters. The existing REAPI transport uses SHA-256 and maps between the
two formats as described below.

The native action result contains:

- **`exit_code`:** signed 32-bit process status; `-1` represents a signalled
  process. Preserve the value, including nonzero codes.
- **`stdout`, `stderr`:** optional Struo digests of captured stream blobs,
  omitted when absent.
- **`outputs`:** map of workspace-relative paths to Struo digests of captured
  output values, omitted when empty.

Store output values as opaque bytes. Struo handles directories, executable
modes, and symlinks; the cache must not extract or rewrite them. Digests
identify uncompressed content, independent of compression used for storage
or transfer.

## Existing REAPI Transport

To serve the current Tuist adapter, implement the following gRPC REAPI v2
storage services in the `build.bazel.remote.execution.v2` package:

- **`Capabilities.GetCapabilities`:** advertise supported compression and
  batch limits. Identity transfers must work without compression support.
- **`ContentAddressableStorage`:** `FindMissingBlobs`, `BatchUpdateBlobs`, and
  `BatchReadBlobs`, using SHA-256 content digests and per-blob statuses.
- **`ActionCache`:** `GetActionResult` and `UpdateActionResult`, storing both
  Struo result records and BLAKE3-to-SHA-256 mappings.

Also implement `google.bytestream.ByteStream.Read` and `Write` for larger
content and file-backed uploads. Support both batch and streaming reads;
not every Struo read path falls back when `BatchReadBlobs` is unimplemented.

No REAPI execution service is required. Struo does not upload REAPI `Action`
or `Command` messages or call `QueryWriteStatus` in this cache transport.

### Discovery and tenant isolation

The Tuist control plane receives:

```http
GET /api/cache/endpoints?account_handle=acme
Authorization: Bearer <token>
x-tuist-feature-flags: kura
```

It returns JSON such as:

```json
{"endpoints":["https://cache.example.com"]}
```

`Cache-Control: max-age` controls persisted discovery freshness; the default
is one hour. Unreachable discovery can reuse a stale answer, but refused or
malformed discovery is not treated as absence. `TUIST_CACHE_ENDPOINT`
overrides discovery only, not authentication, namespace, or serialization.

Every gRPC call carries `authorization: Bearer <token>` and
`x-tuist-account-handle: <account>`. The REAPI `instance_name` is the project
handle, or `default` when no project is set. The account is not prefixed to
`instance_name`. Authorize the token against both account metadata and the
instance; never trust those caller-selected strings as authorization.

Struo resolves `TUIST_TOKEN` or a stored developer/CI session. See
[Tuist authentication](/guide/infrastructure/tuist#authenticate-automation).
A new adapter can use its own discovery and credential flow; it does not
need to copy Tuist's control-plane routes.

### Digest mappings

For a Struo digest `d`, use its lowercase hex text. The adapter constructs two
synthetic REAPI action-cache keys:

```text
blob preimage   = UTF-8("struo.blob.v1")   + byte(0) + UTF-8(hex(d))
action preimage = UTF-8("struo.action.v1") + byte(0) + UTF-8(hex(d))
mapping key    = {hash: hex(SHA-256(preimage)), size_bytes: len(preimage)}
```

The blob preimage is 77 bytes; the action preimage is 79 bytes. The separator
is one NUL byte, not the two characters `\` and `0`. These keys are opaque
action-cache keys; the preimage is not uploaded as a REAPI action.

The blob mapping is a REAPI `ActionResult` with an `output_files` entry named
`blob` whose digest is the SHA-256 and uncompressed byte length of the content.
The actual bytes live in the remote CAS under that SHA-256 digest. Publish the
bytes before the mapping. The empty blob is available locally without a
remote mapping.

The action mapping holds a REAPI `ActionResult` with the exit code, stream
digests, and output-file digests translated to SHA-256. It also preserves the
native result as JSON bytes in
`execution_metadata.auxiliary_metadata`, with `Any.type_url` equal to
`type.googleapis.com/struo.cache.v1.ActionResult`. Despite the `Any` wrapper,
its value is JSON, not a serialized protobuf message.

Preserve this auxiliary metadata unchanged, including its JSON encoding.
Struo checks that the native and REAPI records agree on exit code, output
paths, and stream presence, then checks referenced content availability.
Older records without native metadata require fetching content to recover
the Struo digests.

### Transfer behavior

Small in-memory uploads use `BatchUpdateBlobs` up to 2 MiB; larger or
file-backed uploads use ByteStream. Uncompressed ByteStream resources are:

```text
read:  <instance>/blobs/<sha256>/<uncompressed-size>
write: <instance>/uploads/<upload-id>/blobs/<sha256>/<uncompressed-size>
```

ByteStream reads request uncompressed bytes. If advertised by the service,
Struo can upload Zstandard-compressed content using
`<instance>/uploads/<upload-id>/compressed-blobs/zstd/<sha256>/<uncompressed-size>`.
The digest and size still describe uncompressed bytes. Writes begin at offset
zero, use byte offsets in the transmitted data, and finish with
`finish_write = true`. Retries use a fresh upload id and restart rather than
resume the transfer.

Batch reads must return exactly one response per requested digest, with a
per-blob status. Duplicate, missing, or unrelated responses are protocol
errors. Return only a compressor accepted by the request; Zstandard batch
support is negotiated separately from ByteStream compression support.

## Failure and Retention

- **Missing data:** use `NOT_FOUND` only for absent entries or blobs.
  `FindMissingBlobs` lists absent digests; batch reads report absence in the
  individual blob's status, not as an RPC-wide `NOT_FOUND`.
- **Failed requests:** use `UNAUTHENTICATED` for invalid credentials,
  `PERMISSION_DENIED` for denied access, `INVALID_ARGUMENT` for invalid
  requests, and `UNAVAILABLE` for temporary service outages. Never disguise
  these failures as misses.
- **Eviction:** the service may evict content. Struo treats a result with
  confirmed-missing references as a miss, not a successful restore. A prior
  availability check does not pin content against later eviction.
- **Service policy:** enforce tenant isolation, quotas, and retention on the
  server. Struo's local cleanup and forget operations do not delete remote
  entries. Keep credentials out of logs.

## Struo Adapter Responsibilities

These are client responsibilities, not extra server APIs:

- Keep a local cache and consult it first. Verify downloaded sizes and hashes
  before accepting content locally, and bound transfer memory and temporary
  storage with Struo's shared transfer budget.
- Upload content before its mappings and action result. Make retries safe;
  the current adapter retries some transient failures within a bounded budget.
- Preserve failure categories. Struo may execute locally when the cache is
  unavailable, but does not treat refused access, bad configuration, invalid
  responses, or corruption as ordinary misses.

Remote writes are currently best-effort after local storage succeeds.
Standalone streams larger than 8 MiB are kept local until needed by action
result publication. A successful local write therefore does not prove the
service received anything; verify it from an independent reader.

## Acceptance Checks

Run these checks against your service through the Struo adapter, using
disposable data and two clients with separate local caches. Start with the
[guide's remote-reuse check](/guide/infrastructure/providers#validate-a-cache-without-a-local-false-positive),
then deliberately exercise failures:

1. **Remote reuse:** publish a result on the first client, then retrieve it
   from an empty local cache on the second. Exit code, captured streams, and
   output bytes must match exactly, including optional or empty values.
2. **Eviction:** delete a referenced blob, then look up the result from a
   fresh local cache. Struo must report a miss. Also evict a blob between
   lookup and restoration; incomplete restoration must not count as success.
3. **Failure classification:** compare an absent entry, a stopped service,
   rejected credentials, and denied permissions. Only confirmed absence is
   an ordinary miss; outages and refused access remain distinct failures.
4. **Interrupted and repeated writes:** interrupt a blob upload or result
   publication, then retry. Partial blobs must not be readable, and retrying
   or concurrently writing identical content must remain safe. A second
   client must retrieve a complete result after publication succeeds.
5. **Transfer integrity:** test small and large blobs, identity and negotiated
   compression, truncated data, incorrect hashes, and wrong sizes. The client
   must restore exact bytes or reject the transfer without accepting corrupt
   content locally. Memory and staging must stay within configured budgets.
6. **Tenant isolation:** use the same project handle in different accounts.
   Neither token may read or overwrite the other's entries. Repeat with a
   token lacking access to another project in its own account.
