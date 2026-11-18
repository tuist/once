# Artifacts

**Artifacts are immutable build products. Publishing sends them to your
configured infrastructure.** Struo builds, packages and publishes the content;
the provider decides how people download, install, serve or run it.

An application, command-line program, server, static site, dataset or report
can be an artifact. No container runtime or running web server is required.

## Package explicitly selected content

```toml
[[target]]
name = "site"
kind = "artifact"
srcs = ["public/**/*"]

[target.attrs.interfaces."dev.struo.static-http.v1"]
payload = "content"

[target.attrs.interfaces."dev.struo.static-http.v1".spec]
root = "public"
index = "index.html"
```

`artifact` creates a deterministic ZIP with package-relative paths. Its
`file_mode` defaults to 420 (0644); use 493 (0755) for executable files.
Permissions are declared, not inferred from the packaging host. Custom target
kinds can export existing file outputs directly or declare mixed-mode archives.

```sh
struo query artifacts site
struo artifact export site --output dist
struo artifact publish site --provider local
```

Export returns the immutable root descriptor and a self-contained directory.
Local publication retains the package separately from the action cache and
writes a publication receipt after all content is present. Repeating it repairs
missing or changed content. Deleting the workspace still deletes local
publications; use an external export destination for longer retention.

## Content describes what it is

Each artifact has named payloads, each with a SHA-256 digest, byte size and
media type. A manifest binds payloads to optional versioned interfaces. An
artifact set groups named variants with explicit selectors. Neither target
paths, Git revisions, tracks, credentials nor infrastructure URLs are part of
an individual artifact's content identity.

Exported packages contain `index.json` and `blobs/sha256/<digest>` files.
The root describes an `once.artifact_set.v1` document; members refer to
`once.artifact.v1` manifests, which refer to payloads. JSON object keys use
stable ordering and compact UTF-8 serialization. Consumers verify every
descriptor's digest and size before trusting content.

An interface has `payload` and `spec` fields. Unknown interfaces survive
export and remain downloadable; they never grant permission to execute a
command or dynamically load a handler. Examples of consumption contracts:

- **Download:** No interface required. Retain arbitrary bytes.
- **Static site:** `dev.struo.static-http.v1` declares the archive `root` and
  `index`. Never serve private sibling files.
- **CLI:** A producer-owned executable contract declares entrypoint, arguments,
  OS, architecture and required runtime content.
- **Server:** A producer-owned process contract adds ports, readiness and
  runtime requirements. Secrets and assigned addresses come from infrastructure.
- **Application:** A producer-owned installation contract declares the package,
  platform, runtime compatibility and installation requirements.

Consumption interfaces are declarations. Struo does not implement static hosting,
CLI installation or server activation. An executable alone is not a complete
runtime closure. A compatible consumer must validate its supported contract,
unpack safely, isolate untrusted content, bind secrets with authorization,
allocate resources, and own routing, access, retention and cleanup.

## Choose publication infrastructure

```toml
[infrastructure.artifacts]
name = "local"
```

`artifacts` is the capability being configured; `name` selects its provider.
The built-in `local` provider needs no separate definition. Named provider
definitions can be reused by the capabilities they support.

Cache, execution and artifact bindings are independent. Publication is explicit
unless build publication is opted into and authorized. A cache hit does not
skip publication, and publication does not run the published content.
An absent artifact binding is an error unless `--provider` selects one.
`--track` adds distribution context without rebuilding different content.

Provider-specific configuration, authentication, supported formats and limits
live in each provider's guide. See [Tuist](/guide/infrastructure/tuist#publish-application-previews)
for an artifact receiver that also projects supported applications into previews.

## Publish after successful builds

Select targets in the workspace configuration:

```toml
[infrastructure]
publish_on_build = ["application"]
```

Then explicitly publish and authorize the configured remote destination:

```sh
struo artifact publish application --allow-on-build
struo build application
```

Authorization is local machine state, not repository configuration. It binds
the canonical workspace path, target and exact service endpoint to the stable
destination returned by the service. Changing any of them requires explicit
authorization again. A copied checkout or another machine is not authorized
by the committed policy alone.

After all requested builds succeed, Struo considers only requested targets
listed in `publish_on_build` that expose the artifact capability. The producer
must also approve automatic publication of its entire exported closure through
`struo.artifact_info.v2`. Older v1 declarations default to no approval.
Without the policy or local authorization, builds make no artifact-publication
network requests. A changed stable destination is rejected before content upload.

Remote publication sends the full immutable closure: the artifact set, every
member manifest and every payload, including unknown interfaces. Its intent
contains the root descriptor, target, optional track and Git revision. Before
uploading, Struo looks up the intent's remote receipt and verifies its key, root
and destination. Replaying the same retained intent returns that receipt without
uploading content again. An unconfirmed completion is an error; replay the same
intent to recover its receipt rather than assuming success. Retention and expiry
are receiver policies, not a promise of permanent byte availability.

## Publication service contract

Remote adapters supply an authenticated service endpoint. The protocol is
independent of consumption interfaces and never interprets application metadata:

- `GET <endpoint>` returns `{"schema":"once.publication_service.v1","destination":"<stable destination>"}`.
- `GET <endpoint>/publications/<key>` returns a receipt, 404 for an unknown
  intent, or 410 for an expired intent that cannot be reactivated.
- `POST <endpoint>/publications/<key>/content/<digest>` streams exact bytes as
  `application/octet-stream`. The receiver verifies SHA-256 before acknowledging
  with 204. A committed intent is immutable and rejects further uploads with 409.
- `POST <endpoint>/publications/<key>` commits this JSON intent only after the
  complete reachable closure is durably retained:

```json
{
  "schema": "once.publication_intent.v1",
  "root": {"digest": "sha256:<64 lowercase hex digits>", "size_bytes": 123, "media_type": "application/vnd.struo.artifact-set.v1+json"},
  "target": "application",
  "track": null,
  "revision": null
}
```

`key` is `sha256:` plus the SHA-256 of this complete intent serialized as
compact UTF-8 JSON with recursively sorted object keys. Both optional values
are present as strings or `null`; descriptor sizes are nonnegative integers.
A successful commit or lookup returns exactly `schema`, `key`, `root`,
`destination` and `url`, with schema `once.publication_receipt.v1`. The URL is
absolute HTTPS, except same-origin HTTP for local development. It contains no
embedded credentials. Publication schemas are versioned separately from artifact
and consumption schemas.

Struo uploads each distinct locally exported digest, never server-selected local
files. Responses are bounded to one mebibyte, and redirects are not followed.
Other statuses and malformed or mismatching receipts are errors. An upload
conflict is successful only if a separate lookup confirms the same publication.
The client does not automatically retry ambiguous writes. Different targets,
tracks or revisions produce different intents even when their content is equal.

## Use the same surface from an agent

- `struo_query_artifacts` inspects capability support and the provider shape.
- `struo_export_artifacts` mirrors local export.
- `struo_publish_artifacts` mirrors explicit publication.

The last two require `struo mcp --allow-run`. Authorize the selected destination
before asking an agent to publish. See the [module contract](/reference/modules/#artifact-providers)
to add artifact support to another target kind.
