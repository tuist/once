# Request for Comments 0015: Artifact Publication

## Status

Accepted for implementation

## Decision

Once produces and publishes artifacts. An artifact is immutable content and
typed consumption metadata; a publication adds destination, target, source
revision, access and retention. Infrastructure decides how people download,
install, serve or run the published content. Those experiences are not Once
concepts and do not change artifact identity.

The generic model accepts any payload media type and zero or more named,
versioned consumption contracts. Unknown contracts remain distributable but
must not execute implicitly. Producers declare correctness requirements;
consumers choose compatible runtimes, authorized bindings and resource limits.
Secrets, credentials, signed download URLs and provider placement never belong
in artifact identity.

## Graph contract

Target kinds expose an `artifact` capability and the `once_artifact_info`
provider. Their implementation returns `artifact_info` with schema
`once.artifact_info.v1` and an `artifacts` array. Each declaration has a unique
`name`, named `payloads` containing declared output `path` and `media_type`,
optional typed `interfaces`, and optional variant `selectors`.

An interface key is a namespaced versioned contract identifier; its value is a
JSON object governed by that contract. The generic envelope validates bounds
and structure, not ecosystem semantics. Contract authors publish schemas and
validate requirements in their modules; consumers validate supported contracts
before using the content. An unknown interface is never permission to fetch
or execute a handler.

The built-in `artifact` target packages explicitly selected sources. Other
target kinds can export their own outputs directly. Apple application export
is owned by its Starlark module. Packaging is cacheable graph work; publication
is a separate explicit operation and must run even on a cache hit. Ordinary
builds do not publish or package additional outputs.

## Content and publication

`once.artifact.v1` manifests contain named payload descriptors and interfaces.
Descriptors contain algorithm-qualified digest, byte size and media type.
SHA-256 is the interchange baseline; Once's internal cache hashes do not change.
`once.artifact_set.v1` groups independently addressed artifacts with names and
selectors. Alternatives are not runtime dependencies. Attachments and required
runtime dependencies must not be inferred from names or filesystem layout.

Publication uploads required content before committing the root. A successful
receipt identifies the immutable root and its destination. Local publication
stores a self-contained directory independently of the build cache and commits
its receipt last. Repeating the same local publication is idempotent and checks
content rather than trusting an old receipt. Network providers must distinguish
missing data, authorization failure, incomplete upload and an unknown outcome.
Build events describe publication results; they are not the storage transport.

Transfers stream through bounded buffers and share command resource admission.
Payload paths must name declared outputs inside the workspace, including after
symlink resolution. Metadata has explicit size and cardinality bounds. Archive
creation records declared permissions and symlinks without invoking a shell.

## Command and agent surface

`once query artifacts <target>` inspects the capability and export contract
without executing the build. `once artifact export <target>` builds and writes
a self-contained content-addressed package locally. `once artifact publish
<target>` exports and publishes through the explicitly configured artifact
infrastructure. Stateful MCP tools require `--allow-run` and mirror these CLI
operations. Configuration extends `once.toml`, not a parallel manifest.

## Tuist compatibility

Tuist's existing preview multipart API accepts app bundles, IPAs and APKs,
not arbitrary manifests. A compatibility adapter may publish an artifact whose
module supplied the `dev.tuist.preview.v1` interface. The interface identifies a
payload and a request metadata file. The adapter treats native metadata as
opaque parameters and implements only Tuist's transport. It does not parse
executables, recognize toolchains or rebuild products.

The legacy API cannot promise generic artifact retention, atomic variant
publication, or idempotent retries. In particular, duplicate-build conflicts
are errors, not evidence that a usable publication exists. These limitations
are surfaced rather than hidden. A generic Tuist publication service needs
server support for immutable artifact descriptors, explicit grouping,
idempotent commit receipts and retention independent of cache eviction.

The proposed [provider-neutral publication protocol](0016-artifact-publication-protocol.md)
specifies that follow-up boundary, including Once-owned opt-in automation.
It is not implemented by the compatibility adapter described here.

## Consumer responsibilities and non-goals

Infrastructure owns installation, activation, isolation, routing, secret bindings,
resource allocation, leases and cleanup. Publishing does not start compute or
grant access to a device or secret. Public static roots must be explicit and
served from isolated origins. Device provisioning and code-signing eligibility
remain consumer checks. Re-signing creates different immutable content.

This change does not implement a hosting scheduler, a universal installer,
production promotion, OCI registry transport, or dynamic handler loading.
OCI descriptors and artifact manifests are useful precedent, not a mandatory
container runtime or registry dependency.

## References

- [OCI artifact manifests](https://github.com/opencontainers/image-spec/blob/v1.1.1/manifest.md)
- [OCI distribution](https://github.com/opencontainers/distribution-spec/blob/v1.1.1/spec.md)
- [Buildpacks launch contracts](https://github.com/buildpacks/spec/blob/main/buildpack.md)
- [Nix runtime closures](https://nix.dev/manual/nix/2.19/glossary#gloss-closure)
- [Vercel explicit output boundaries](https://vercel.com/docs/build-output-api/primitives)
- [Tuist previews](https://tuist.dev/en/docs/guides/features/previews)
