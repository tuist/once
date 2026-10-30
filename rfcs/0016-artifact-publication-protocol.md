# Request for Comments 0016: Provider-neutral artifact publication protocol

## Status

Proposed. This document specifies a contract, not shipped commands or server
support. It extends RFC 0015; it does not replace the immutable package format.

## Problem

Once must own publication, including opt-in publication after a successful build.
A Tuist command that wraps a Once invocation is the wrong boundary. So is
making every producer emit a native Tuist preview request. Applications are one
artifact category, and Tuist Previews is one consumer experience, not the
publication protocol.

The existing Tuist compatibility adapter uploads one selected payload through
the legacy preview multipart API. It does not retain a self-contained package
and cannot provide atomic sets or retry-safe receipts. A duplicate response
does not confirm that usable content exists. It must not be used as a fallback
for the protocol proposed here.

## Decision and ownership

- **Producers** own packaging, variant selectors, content metadata, runtime
  closure and signing requirements. They use the artifact capability and typed
  interfaces, not provider request bodies.
- **Once** owns export, publication policy, authentication, bounded transfer,
  retries and confirmed receipts. Publication runs outside cached graph actions.
- **Providers** own durable storage, authorization, access, retention and
  idempotent commits. They retain unknown interfaces without interpreting them.
- **Consumers** own compatibility checks and experiences such as downloading,
  installing, hosting or running. Publication never activates content.

Tuist uses its existing project authentication and distribution authorization.
Its preview experience projects compatible published applications. It is not a
second upload performed by Once and must not become part of artifact identity.

## Preserve the content envelope

Preserve `once.artifact_info.v1` for existing explicit exports and reuse
`once.artifact.v1` and `once.artifact_set.v1` for immutable content. Automation
adds a v2 producer declaration described below, not a new content format. The
root addresses the entire set, each member addresses an artifact manifest,
and each manifest addresses its named payloads. The server stores these exact
bytes and verifies SHA-256 digests and byte sizes. Re-encoding a manifest changes
its identity and is not allowed during transport.

Payloads accept any media type. Interfaces are optional, namespaced and
versioned; their `payload` points to a named payload and their `spec` is owned
by the contract author. Multiple interfaces can describe the same payload.
Selectors express alternatives, never dependencies or instructions to execute.
No global `application | website | executable` enum is introduced.

| Artifact | Producer-owned content and interfaces | Possible consumption |
| --- | --- | --- |
| Apple application | Bundle ZIP, platform/SDK/architecture, bundle identity, minimum OS and signing requirements | Install on a compatible simulator or authorized device |
| Android application | APK or explicitly modelled split set, package identity, SDK/ABI and signing requirements | Install on a compatible Android runtime |
| Static site | Archive with an explicit public root and index | Serve only the declared root from an isolated origin |
| Command-line program | Executable and runtime content, OS/architecture and entrypoint | Download or install with user authorization |
| Server | Runtime content, entrypoint, port/readiness and resource requirements | Activate separately with authorized runtime bindings |
| Dataset, report or symbols | Arbitrary named payloads; no execution interface required | Download, inspect or associate with another artifact |

These are contract categories, not implementations claimed by this RFC.
Supporting a new media type or interface does not require changing generic
Rust, CLI, HTTP routing or the publication schema. An incompatible interface
change gets a new contract identifier; consumers never guess a newer version.

Every required runtime byte must be a declared payload reachable through the
root. A producer can package a complete runtime closure into an archive or use
named payloads referenced by its interface. Generic v1 does not resolve external
runtime dependencies. An OCI layout can itself be a payload without making an
OCI registry or container runtime mandatory for all artifacts.

Arbitrary digest-looking strings inside an unknown `spec` are not graph edges.
Cross-artifact attachments, required dependencies and attestations need an
explicit, versioned relationship contract before consumers can follow them.
Do not infer relationships from member names, selectors or archive layout.
Future structural edges require a new envelope version and negotiated support;
they must not be smuggled into an optional extension that old servers ignore.

Credentials, provisioning secrets, signed URLs and assigned runtime addresses
are never payload metadata. Public certificate fingerprints and non-secret
signing constraints are content facts. Re-signing changes bytes and therefore
produces a new immutable artifact.

## Publication intent and identity

A publication adds distribution context without changing the immutable root.
The v1 intent is an object with these fields:

| Field | Meaning |
| --- | --- |
| `schema` | `once.publication_intent.v1` |
| `root` | Complete root descriptor: `digest`, `size_bytes`, `media_type` |
| `target` | Workspace-relative logical target identity, not an absolute host path |
| `source` | Optional object containing `repository`, `revision` and `dirty`; unknown values are omitted, not invented |
| `track` | Optional distribution label, not a mutable alias or promotion instruction |

The receiving destination is selected by the authenticated endpoint, never by
untrusted content. Run IDs, timestamps, signed URLs and machine-local paths do
not belong in the intent. Runs can be associated separately with an existing
publication without creating another publication or modifying its identity.

An intent key is `sha256:` followed by the lowercase digest of its RFC 8785
canonical JSON bytes. Missing optional fields are omitted, not serialized as
null. Sizes must be nonnegative integers within the interoperable JSON range
(at most 9007199254740991). No arbitrary extension fields are accepted in a v1
intent. Canonicalization applies only to the intent, not existing content blobs.

Capture source context once, before execution, and use that snapshot for both
cached and executed results. `revision` is the full lowercase Git commit object
ID when available, never a moving branch name. `dirty` is a boolean when known.
`repository` is an absolute credential-free HTTPS repository URL, with no
userinfo, query, fragment or trailing slash. Strip a terminal `.git`; translate
recognized SSH remotes to that form only when the host/repository mapping is
unambiguous. Omit unrepresentable or local remotes instead of recording private
paths or guessing identity. Partial source objects are allowed; omit an empty
object. These are normalization rules, not proof of repository ownership.
Different available source context can intentionally produce a different
publication even for identical bytes. Deduplication promises the same intent,
not equivalence between all Git remote aliases or source-discovery failures.

Identity is scoped to the authenticated destination plus this key. Consequently:

- A cached build and its original build publish the same root and intent once.
- Concurrent publishers of the same intent receive the same publication.
- A different root, target, source context or track creates a different intent.
- Credentials and provider URL changes do not mutate artifact content.
- A new run observes an existing publication instead of silently skipping it.

The provider recomputes the key and rejects a mismatch. Source context is a
client assertion, not verified provenance. Missing or dirty source metadata does
not make an artifact trusted. Access and retention are destination policy,
not content fields or permissions a producer can grant itself.

## Provider protocol

The following paths are relative to a provider's authenticated artifact service
base. The Tuist adapter resolves that base within the selected account/project
and reuses existing credentials. Other providers can map the same operations
without depending on Tuist names or its preview API.

### Discover

`GET /capabilities` advertises `once.artifact_publication.v1`, the stable
destination identifier, supported root schemas and digest algorithms, maximum
blob size, maximum closure byte count, metadata/closure limits and supported
transfer profiles. The baseline transfer profile is authenticated streaming
HTTP `PUT`. A provider can additionally offer a separately versioned
direct-storage or multipart profile without changing artifact identity or
commit semantics.

No support, an unknown protocol version, or insufficient limits is an explicit
unsupported result before uploading. It never triggers a legacy preview upload.
An absent endpoint must not be interpreted as an empty successful publication.

After approval of discovery, every subsequent request carries an
`Once-Destination` header with that stable destination identifier. Before
accepting content or reading publication state, the service verifies that the
authenticated endpoint still resolves to that identifier; mismatch returns
409 `identity_conflict`. This fences project-handle reuse or rebinding between
discovery and upload. Sending discovery credentials itself requires an approved
service origin; repository configuration cannot authorize a new origin.

### Transfer immutable content

`HEAD /blobs/sha256/{hex}` checks for verified retained bytes in this destination.
`PUT /blobs/sha256/{hex}` streams bytes with a declared content length. Success
means the provider has verified the digest and size and retained the bytes, not
merely issued an upload URL. An identical verified blob is an idempotent success;
a digest or size mismatch is an error. A lost response can be resolved with
`HEAD` or by repeating `PUT` with the same bytes.

The baseline stores opaque blobs. Media types belong to descriptors and can
differ for identical bytes; a blob record must not force one global media type.
`HEAD` reports the verified byte size, and the client compares it to the
descriptor. Neither operation can reveal another project's private content or
existence. Deduplication across projects, if any, must not bypass authorization
or create an existence oracle.

Use TLS and no credential-bearing cross-origin redirects. Any optional signed
storage transport must keep service bearer credentials off storage requests,
validate the transport origin/scheme, bound response bodies, and confirm upload
completion through the authenticated service. Signed URLs never enter receipts,
logs, manifests or cached action results.

Transfers stream with bounded buffers under Once's command resource admission.
Do not read whole payloads into memory, start unbudgeted fan-out, or spool
unbounded response bodies. Client and server enforce descriptor, byte, depth,
count and time limits, including limits on aggregate closure size. The existing
one-mebibyte metadata bound and member/payload cardinalities remain upper bounds;
providers may advertise stricter limits. A server must bound a request before
allocation, not after parsing an oversized body.

### Commit the root atomically

`PUT /publications/{intent_key}` receives the canonical intent with media type
`application/vnd.once.publication-intent.v1+json`. Before acknowledging, the
provider authenticates and authorizes the destination, verifies the key, parses
the root and manifests with the supported schemas, and verifies the entire
declared closure. Interface payload references must resolve. Unknown interfaces
are retained as bounded data, not rejected or executed.

A successful commit atomically creates or returns the same retained publication
and returns `once.publication_receipt.v1` with:

- `publication_id`, stable within the destination;
- `intent_key` and the complete `root` descriptor;
- `destination`, a stable provider-assigned destination identifier;
- `generation`, a monotonic retention generation for this publication;
- `committed_at` and `expires_at` (null only for policy-defined indefinite retention);
- `url`, the authorized provider distribution page, not a signed blob URL.

The caller checks schema, key, root and destination against the request before
persisting a receipt. Successful HTTP with a missing, malformed or mismatched
receipt is an unknown outcome, never success. The first creation can return 201;
an existing valid publication returns 200 with the same receipt. Generic 409
responses are not a substitute for a receipt.

If content is missing, return 422 with code `missing_content` and a bounded
page of descriptors that need uploading. The client uploads that page and
repeats the same commit; pages must make progress or fail within the command's
time/retry budget. Every requested descriptor must match the client's locally
validated root closure; a service response is not permission to upload other
local content. No partial artifact set is visible. Invalid schemas,
oversized graphs or corrupt content fail without committing anything.

The service persists the commit and any required projection work before
acknowledging. Shared durable coordination must preserve these invariants across
replicas, concurrent commits and process failure. Cache eviction cannot remove
retained publication content. Uncommitted uploads can expire, but garbage
collection must serialize with commit so it cannot delete newly pinned blobs.

### Wire objects and limits

These are normative field names and types. JSON examples illustrate shape;
digests shown as repeated zeroes are not test vectors for real content. All
objects reject duplicate keys. V1 intent, capability, receipt and error envelopes
reject unknown fields; new required semantics need a new schema version.
Interface `spec` values remain governed by their independently versioned
contracts, not this envelope's field policy.

Discovery returns 200 and this object:

```json
{
  "schema": "once.artifact_publication.v1",
  "destination": "provider:project:123",
  "root_schemas": ["once.artifact_set.v1"],
  "digest_algorithms": ["sha256"],
  "transfer_profiles": ["once.http-put.v1"],
  "limits": {
    "metadata_max_bytes": 1048576,
    "artifacts_max": 128,
    "payloads_per_artifact_max": 128,
    "interfaces_per_artifact_max": 32,
    "selectors_per_artifact_max": 32,
    "blob_max_bytes": 10737418240,
    "closure_max_bytes": 107374182400,
    "closure_max_blobs": 16513,
    "missing_descriptors_max": 128
  }
}
```

All fields are required. Identifiers are nonempty strings of at most 256 UTF-8
bytes, without whitespace or control characters. Limit values are positive
integers in the interoperable JSON range and
are byte counts unless named for a count. `metadata_max_bytes` bounds each root,
manifest, request and response document, not their sum. Effective limits are
the minimum of client, format and provider limits. Closure accounting counts
each unique digest once, including root, manifests and payloads. Repeated
descriptors must agree on size; media types can differ. The v1 format's maximum
is 1 root + 128 manifests + 16384 payloads, or 16513 unique blobs. Receivers
must also validate the v1 name, reference, selector and interface bounds.

A confirmed commit returns 200 or 201 and this receipt:

```json
{
  "schema": "once.publication_receipt.v1",
  "publication_id": "publication-456",
  "intent_key": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
  "root": {
    "digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    "size_bytes": 123,
    "media_type": "application/vnd.once.artifact-set.v1+json"
  },
  "destination": "provider:project:123",
  "generation": 1,
  "committed_at": "2026-10-29T12:00:00Z",
  "expires_at": "2026-11-28T12:00:00Z",
  "url": "https://provider.example/artifacts/publication-456"
}
```

All fields are required. `generation` is a positive interoperable JSON integer;
timestamps are UTC RFC 3339 strings, with `expires_at` strictly later than
`committed_at` or null for indefinite retention. Digests are `sha256:` plus
exactly 64 lowercase hexadecimal characters. Descriptor sizes are nonnegative
interoperable JSON integers. The URL is absolute HTTPS, with no credentials,
query or fragment. Compare `destination` to the approved discovery result, not
the mutable provider alias. A provider must not reuse a destination identifier
for a different project after deletion or account migration.

Errors use this shape, with `missing` present only for `missing_content`:

```json
{
  "schema": "once.publication_error.v1",
  "code": "missing_content",
  "message": "Required content is not retained",
  "missing": [
    {
      "digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
      "size_bytes": 123,
      "media_type": "application/vnd.once.artifact-set.v1+json"
    }
  ]
}
```

`schema`, `code` and `message` are required. `message` is a nonempty string
bounded to 4096 UTF-8 bytes and carries no secrets. `missing` is nonempty, bounded
by `missing_descriptors_max` and the document limit, and contains distinct
digests in the client's known closure. Recovery makes progress only when at
least one requested digest not already successfully transferred is supplied.
A repeated page without progress exhausts a bounded retry budget, not an
unbounded loop. There is no opaque cursor or authority to fetch unrelated files.

Status/code mappings are 400 `invalid_request`, 401 `unauthenticated`,
403 `forbidden`, 404 `not_found`, 410 `expired`, 413 `limit_exceeded`,
422 `missing_content` or `invalid_content`, 429 `rate_limited`, and
503 `unavailable`. The 422 `missing_content` response permits the bounded
upload-and-recommit continuation defined above. Only 429, 503 and transport
failures permit automatic transient-error retries; unknown codes and malformed
bodies fail closed. A lost commit response still requires receipt recovery.
409 `identity_conflict` is never success. Blob HEAD returns
200 with verified `Content-Length` or 404, with 401/403 distinct from absence;
successful blob PUT returns 204 only after verification. All status paths remain
subject to current authorization and bounded bodies.

### Recover and expire

`GET /publications/{intent_key}` returns the confirmed current receipt, 404 for
no publication in this destination, or 410 when the publication is no longer
retained. It requires current authorization; a saved receipt is not an access
token or evidence that bytes still exist.

When the commit response is lost, Once reads this endpoint and validates its
receipt or repeats the same idempotent commit. A 401/403 is not absence and must
not cause a fresh upload. Retryable failures are bounded; timeout or exhausted
recovery produces an explicit unknown outcome, without a successful receipt.

After expiry or policy deletion, reusing the same key must revalidate and retain
the entire closure before succeeding. It can reactivate the same publication ID
with an incremented generation and a new receipt. A stale receipt alone cannot
resurrect content. Concurrent reactivations create one generation. If policy
forbids re-publication, return an explicit denial rather than a stale success.
Deletion, legal holds and retention overrides remain provider-authorized
operations; publication cannot override them.

### Retrieve retained content

`GET /publications/{intent_key}/blobs/sha256/{hex}` returns exact retained bytes
only when that digest is reachable through this committed publication's root.
It requires current distribution/read authorization, independently of upload
permission, and never exposes staged, expired or unrelated blobs. Clients fetch
the root descriptor from the confirmed receipt, then verify digest and size at
each step. Serving bytes does not authorize unpacking or execution.

Providers can use their existing authorized download mechanisms behind this
operation, including short-lived storage redirects without service credentials.
Private access must remain revocable under the provider's existing bounded
download-URL lifetime. Browser delivery of arbitrary payloads uses download
disposition or an isolated content origin, never the authenticated dashboard
origin with active MIME interpretation. Consumer-specific serving, such as a
static site's public root, requires separate authorization and isolation.

## Once-owned opt-in automation

Ordinary builds remain publication-free by default. A configured artifact
provider alone is not consent to publish on build. The proposed configuration
extends `once.toml`, not a Tuist wrapper or another settings file:

```toml
[infrastructure.artifacts]
name = "tuist"

[infrastructure.artifacts.on_build]
targets = ["mobile/App"]
interfaces = ["dev.once.apple-install.v2", "dev.once.android-install.v1"]
```

This syntax is proposed and not accepted by the current parser. The interface
identifiers select contracts, not toolchains in Rust. Listing targets and
interfaces explicitly authorizes publication of matching top-level build
outputs to the selected destination. Configuring a provider for caching or
reporting does not enable this policy. An absent policy or an explicit empty
target list disables it. `once artifact publish` remains explicit publication,
independent of automatic policy.

`interfaces` is an optional restriction: when present, every member must match
at least one listed approved interface, and an empty list selects nothing.
Omitting it allows interface-free downloads as well as typed artifacts, but
still requires explicit target consent and full producer approval. This does
not permit an application producer to ignore its own signing-related denials.

After a successful Once build, Once's orchestration layer evaluates policy for
requested top-level targets, never every dependency. It requests the artifact
capability for eligible targets, reusing cached execution and packaging where
possible, then publishes outside the graph. Publication credentials, destination
and policy never enter action fingerprints or target outcomes. An ordinary
build without this policy does not add packaging actions. Export and query do
not trigger automatic publication recursively.

Policy matches exact normalized target identities and exact interface versions,
not substring labels or inferred filename extensions. Every member must satisfy
any interface restriction and carry the producer approval defined below. A
target lacking artifact capability or an eligible member is reported as skipped
with a reason, before network transfer.

Publication remains atomic for the root being published. Once must not silently
drop ineligible members or strip private payloads from an exported set. If a
producer mixes distributable and non-distributable members, automatic publication
skips that root and reports why. A producer wanting separate publication units
must expose separate export targets. This keeps selection from silently
rewriting immutable content or leaking sibling payloads.

### Producer approval boundary

Automation requires the proposed `once.artifact_info.v2` declaration envelope.
It retains v1 declaration fields and adds a required `automatic_publication`
object to each declaration:

```json
{
  "approved_interfaces": ["dev.once.apple-install.v2"],
  "approved_payloads": ["bundle", "symbols"],
  "denials": []
}
```

All three fields are required arrays. Interface and payload names must be
unique and refer to that same declaration; the existing 32-interface and
128-payload bounds apply. `denials` holds at most 32 objects, each with a
nonempty `code` of at most 128 bytes and a nonempty `message` of at most 4096
bytes. Unknown fields fail validation. Metadata remains bounded to one mebibyte.

The Starlark producer evaluates its own supported consumption contracts against
the actual declared outputs and emits this result. Generic Rust does not load
interface handlers, inspect native packages or interpret signing modes. Once
accepts automation only when every member has no denials, satisfies any interface
restriction, and `approved_payloads` is exactly the complete set of that member's
payload names. An interface-free download can have empty `approved_interfaces`.
An interface match alone never approves unreferenced payloads. Approval explicitly
covers ancillary metadata and symbols as well as the primary binary, so a private
sibling payload prevents publication unless the producer expressly approves
distributing it.

Missing approval, a v1 cached provider, an unsupported approval version or
missing contract facts fails closed for automation. The approval is tied to
the same evaluated provider and materialized outputs used for export, never
inherited from dependencies or reused for a different root. Exported immutable
root/manifests stay v1; orchestration approval is not a content permission or
artifact identity field. Explicit export/publication continues to accept v1
declarations without asserting automatic eligibility.

Approval is a producer assertion about publication, not universal installability
or proof of artifact trust. Consumers must independently verify supported
contracts, native signatures and current runtime/device authorization. Adding
mandatory signing facts to the existing Apple installation contract requires
`dev.once.apple-install.v2`; old v1 facts are not retrospectively sufficient.
The Android contract and any future contract must specify their producer checks
before their interfaces can appear in approvals. Private inputs must never be
included in a payload merely because approval metadata is absent or ignored.

The same policy runs after cache hits and misses. A cache hit is not a reason
to skip the commit, and a local attempt marker is not evidence of a remote
publication. The provider's verified intent identity prevents duplicates and
repairs expired content. Cancelled or failed builds do not start automatic
publication. A failure after build success is reported separately as a
publication failure, with a nonzero command exit status; it never relabels
cached actions as failed. Explicit publish failures also return nonzero.

Repository configuration is not machine consent. Current provider login grants
credentials and the MCP `--allow-run` gate permits execution; neither supplies
a destination-scoped automatic-publication approval. That approval mechanism
must be implemented before automation is enabled, not assumed to exist.

Approval must bind the workspace identity, resolved service origin and stable
destination, plus the selected targets and interfaces. A repository policy or
destination change invalidates it. Store approval as machine-local authorization
state, not a committed setting or cached graph result, and provide a local deny
that wins over repository opt-in. Use Once's existing credential acquisition,
with no implicit login during a build. Noninteractive execution needs explicit
preauthorization and credentials; it must never answer a prompt affirmatively
on the user's behalf. Policy and consent implementation must be reviewed
together before enabling this configuration.

MCP build tools must disclose that an approved policy can cause publication and
advertise open-world side effects, not retain a purely local build annotation.
The existing `--allow-run` gate is still required but is not publication consent.

## Application producers and Tuist consumption

Apple currently exports a stable `product` member and a bundle ZIP with
`dev.once.apple-install.v1`. Its proposed v2 contract needs explicit
producer-verified signing/distribution requirements before device publication is
eligible. Automation must select v2, not the existing v1 contract. An
ad-hoc device bundle is not made installable by publication. Simulator, desktop
and device eligibility must not be conflated. The native Tuist metadata payload
can remain for explicit legacy compatibility but is not required by this
protocol or by new producers.

Android currently exposes build/run, `android_application` and `android_apk`,
but no artifact capability or artifact provider. Debug and unsigned APK build
support does not establish artifact publication support. Its module must add
`once_artifact_info`, a stable `product` member and a versioned Android install
contract describing the actual package, SDK/ABI and verified signing facts.
Unsigned APKs cannot match an installation publication policy. Split APKs require
an explicit install-set contract, not multiple unrelated alternative members.

The Tuist server retains generic content first. A durable idempotent projection
then makes supported, eligible application contracts available in Previews using
existing access and distribution behavior. Unknown artifact types remain
downloadable; they do not acquire a preview entry or executable handler. A
projection failure is observable independently of storage commitment and does
not make the client re-upload or falsely claim an application is installable.
The generic publication URL reports storage and consumer status without
requiring every artifact to have a preview URL.

Projection work and results carry `(destination, publication_id, generation)`.
Before exposing a result or updating consumer state, workers atomically fence
against the current retained generation and current authorization. Expiry or
deletion makes existing projections inaccessible immediately through a retained
state check, even while durable invalidation/cleanup is still pending. Stale
work cannot recreate a deleted preview, suppress a reactivated generation or
overwrite newer consumer state. Consumer byte access reauthorizes and must not
outlive publication retention through a detached copy, unless a separately
authorized consumer-retention contract explicitly permits it. This does not
promise revocation of copies users have already downloaded.

## Compatibility and implementation boundaries

Keep the current explicit legacy adapter behavior until the generic receiving
service and Once transport exist together. Do not advertise protocol support
before server-side storage, recovery and authorization work. Never silently
upgrade legacy commands to a new destination or retry their non-idempotent
uploads under these rules. Deprecating the compatibility interface is a separate
migration with user-facing documentation and a supported replacement.

Implementation requires the generic receiving service and retention model,
Once transport and validated receipts, Once policy/trust support, producer
contracts, and Tuist's application projection. Tests using a fake HTTP service
establish the client protocol only, not server durability or native eligibility.
This RFC does not itself implement any of those pieces.

## Acceptance and adversarial validation

Before enabling automatic publication, validate at least:

1. Arbitrary download content, static content, a native executable and a
   multi-member set round-trip byte-for-byte, preserving unknown interfaces.
2. Android signed/unsigned and Apple simulator/device/signing-ineligible outputs
   exercise producer contracts. Native builds and install tests are distinct
   from mocked metadata tests; Linux does not validate Apple signing.
3. Cache hit and miss produce one retained publication for the same intent;
   concurrent clients, lost commit responses and replica changes do not duplicate it.
4. Disabled/absent policy, dependencies, failed/cancelled builds, unsupported
   capabilities and denied consent send no publication requests.
5. Expired content, stale receipts, partial uploads, malformed confirmations,
   corrupt digests, missing payloads and conflicting sizes never yield success.
6. Authorization failure, another project's digest and cross-origin redirects
   cannot disclose content or credentials. Private sibling payloads are never
   silently published by filtering an exported set.
7. Retention/GC racing with commit, replica failure and projection retries
   preserve durable content and idempotency. Mocked HTTP tests are insufficient
   for these service invariants.
8. Oversized metadata, many small payloads, large streams, repeated missing-content
   responses and unknown versions stay within byte, memory, request and time limits.

## Alternatives rejected

- **Wrap `once build` in Tuist:** puts policy and side effects outside Once,
  duplicates orchestration and cannot define a general producer contract.
- **Use the legacy preview multipart API as generic publication:** drops content
  and cannot establish atomic sets, safe retries or retention.
- **Publish in a cached graph action:** cache replay skips required side effects
  and puts credentials and destination into the wrong boundary.
- **Treat the first supported member as the product:** loses variants and can
  publish the wrong or signing-ineligible output.
- **Infer support from filenames or execute unknown contracts:** bypasses
  producer ownership and turns metadata into an execution/security boundary.
- **Force OCI on every artifact:** adds registry/runtime requirements to data,
  reports, websites and native applications that do not need them.
