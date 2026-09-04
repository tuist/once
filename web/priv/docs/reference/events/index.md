# Live Run Event Protocol

The Once client streams typed events over gRPC to a compatible ingest
server as they happen during a run: run and target lifecycle, per-target
phases, cache decisions, and (framework-dependent) individual test
cases. The protocol is versioned as `once.events.v1`.

This page documents the shape of the wire, the event catalog, opt-in
configuration, the delivery guarantees the client honours, and the
current scope of what a Once run emits today.

## What the client sends

Every run publishes a stream of events to a single ingest service. The
service defines four methods and one bidirectional streaming call:

- `GetServerCapabilities` returns supported protocol versions, batch
  and event size caps, the finalization grace window, deduplication
  retention, and the safe-literal allowlist version.
- `GetArgvHashKey` returns the project-scoped keyed-hash key the
  client uses to normalize argument values without leaking their
  content. The key rotates on a server policy; retired keys remain
  valid on ingest for a documented grace window.
- `PublishRunEvents` is the bidirectional streaming call. The client
  streams batches; the server streams one `BatchAck` per batch.
- `GetRunAck` is a unary read-only probe used on reconnect to
  discover the server's durable state before the client resumes.

Every event carries a monotonic sequence number, a wall-clock stamp for
display, and a monotonic offset from `run.started` that is authoritative
for durations. A batch carries a client-generated id, an optional list of
loss-interval control records, and a contiguous range of events. The
server processes any loss controls first, then validates the batch's
sequence range against the current expected next sequence, then durably
projects the events under a single transaction that also advances the
acknowledged sequence.

## Delivery guarantees

The delivery model is at-least-once with server-side deduplication by
`(run_id, seq)`. Duplicates with identical payload are silently accepted;
duplicates with divergent payload are logged and dropped. The client keeps
an in-memory ring of unacknowledged events with a reserved slot for the
run's terminal event, so overflow of ordinary events can never suppress
the run's terminal state.

Ordinary overflow drops the oldest unacknowledged events and records the
dropped sequence range as a loss interval. On the next batch send, the
loss interval is drained into the batch's `gap_advances` list as a
canonical sorted non-overlapping set. Loss is never carried as an event
that consumes a sequence number, so a dropped range cannot collide with a
retained event's sequence.

Terminal delivery uses an explicit intent event. When the run is ending,
the client emits a finalization intent event and enters a bounded local
drain. The server starts its own grace window on receipt and transitions
the run to a distinct finalization state: finalized on receipt of the
terminal event, finalization pending if the grace elapses first, or lost
if no intent and no heartbeats arrive past the liveness timeout. A late
terminal event within the deduplication retention window transitions a
pending or lost run to finalized while preserving the fact that the run
was previously marked pending or lost.

Reconnect uses `GetRunAck` to reconcile the client's mirror of the
server's durable state before resuming. Ring buffer and loss intervals
survive across reconnects because they live in the caller-owned session
state; producers continue publishing through the outage without observing
it.

## Event catalog

Every event carries scope information sufficient for the projector to
resolve per-target and per-case correlations without ambiguity.

### Run lifecycle

- `run.started` records Once version, protocol version, host class,
  git revision, a normalized argument vector, working directory
  relative to the workspace, an environment fingerprint, a graph
  digest, the project identifier (validated against the bearer
  token), and the effective limits negotiated with the server.
- `run.finalizing` announces the client's intent to close the run
  and starts the server's grace window.
- `run.completed` records the terminal result (succeeded, failed,
  cancelled, timed out, or infrastructure error), an optional
  cancellation reason, the wall duration, and aggregate totals.
- `run.heartbeat` fires every five seconds while the run is active
  so the server can distinguish a network partition from a clean
  exit.

### Graph

- `graph.compiled` records the graph digest, target count, target
  kind histogram, and the requested roots.
- `target.instance` records a target instance identifier for the
  logical occurrence of a target within a run.

### Target execution

- `target.queued` and `target.started` bracket a target attempt.
- `target.phase` records transitions along the six exclusive
  ordered phases: queued, cache checking, preparing, executing,
  capturing, and publishing.
- `target.wait` announces what a queued target is waiting on
  (dependency wait, resource wait, worker wait, throttled, or
  infrastructure).
- `target.completed` records terminal result, whether a cache
  restore covered the outcome, an optional exit code, an evidence
  reference, and duration.
- `target.cancelled` records a cancellation reason.
- `target.retried` links a previous attempt identifier to the new
  attempt identifier a following `target.queued` will mint.

### Tests

- `test_suite.started` and `test_suite.completed` bracket the tests
  produced by one test target.
- `test_case.started` and `test_case.completed` describe every
  individual case with a stable case identifier, its display name,
  and per-case duration and terminal result.
- `test_case.retried` links previous and new attempts across a
  case retry.

### Logs

- `log.chunk` streams a slice of standard output or standard error
  scoped to the run, a target execution, or a test case execution.
  Chunks in a scope never overlap.
- `log.truncated` records dropped bytes when a stream exceeds its
  size threshold.

### Cache

Cache is a first-class concern in this protocol; every event in one
lookup is grouped by a single cache decision identifier so the
projector renders one cache decision row per target execution.

- `cache.probe` records a single tier lookup with its ordinal and
  outcome (hit, miss, error, or bypassed).
- `cache.miss_reason` records why an action digest did not match a
  baseline. A required resolution field states whether a comparison
  baseline is present (with a reference to a previous run and
  digest), none is available, none was attempted, or the lookup
  failed. A primary reason plus the full set of detected reasons,
  differing input paths (with a truncation indicator), and an
  analysis status accompany the resolution.
- `cache.upload`, `cache.download`, and `cache.store_reused` each
  record a single content transfer or a skipped-upload signal.
  `cache.store_reused` carries the bytes an upload would have
  transferred had the digest not already been present.

### Artifacts and diagnostics

- `artifact.published` announces a declared output with its kind, a
  content reference, and a workspace-relative path.
- `diagnostic.emitted` carries a normalized lint or type-check
  finding with a stable fingerprint for cross-run deduplication.

## Identity and redaction

Tenancy is derived from the bearer token; the wire never carries a
tenant field. The project identifier in `run.started` is validated
against the token's authorized set.

Raw absolute paths, raw environment values, raw hostnames, and
unredacted argument vectors are not part of the protocol; the schema
has no fields for them. The client sends only the sanitized shapes:

- A normalized argument vector as an ordered list of safe literals
  (from an explicitly enumerated allowlist), flag keys, combined
  `--key=value` records with an opaque value hash, and generic
  opaque value hashes for every other positional. The keyed hash
  uses a project-scoped key retrieved via `GetArgvHashKey`.
- A repository-relative working directory validated for traversal
  at ingest. Leading `/`, `..`, or absolute-path shapes are
  rejected.
- Coarse host and worker class tokens rather than resolvable
  hostnames.
- A content-addressed environment fingerprint over the actionable
  subset of the environment.

Log ingestion is off by default and requires explicit per-project
opt-in on the server. Diagnostic messages, test names, parameters,
and artifact paths remain author-controlled but are validated at
ingest; suspicious content (absolute paths, secrets-shape tokens)
is quarantined and surfaced as a data-quality warning on the
projected state.

Every event field carrying a source path (test case files, diagnostic
locations, artifact paths, differing inputs) must be workspace-relative
and is validated for traversal at ingest. Violations clear the offending
field and record a data-quality warning; the enclosing event is still
projected.

## Enabling the client

Live ingest is a **provider capability**. Configure a provider by its
origin in `once.toml` and authenticate against it; the client then
asks the provider what it supports and uses the events endpoint the
provider advertises.

The workflow is:

1. The workspace names a provider (for example a provider bound to
   `https://tuist.dev`) in its infrastructure configuration, and the
   user authenticates against it.
2. On run start the client asks the provider for its capabilities.
   The provider advertises which of `cache`, `remote_execution`, and
   `events` it supports, along with the endpoint (and any auth
   metadata) for each supported surface.
3. If `events` is advertised, the client dials the endpoint the
   provider named, using the same bearer credentials the provider
   already established. No separate configuration is needed.
4. If `events` is not advertised, the client simply does not send
   any events. The run behaves exactly as it did before ingest was
   available.

This makes ingest a property of the provider rather than a
per-project deployment concern: turning it on for a workspace is a
matter of switching to a provider that advertises it.

The client subscribes to the run's local event bus, streams events
through a bidirectional call while the run executes, and drains for
up to three seconds before the CLI exits. If the endpoint is
unreachable or the connection is lost, the client backs off, calls
`GetRunAck` to reconcile, and resumes. Ingest failures never abort
the run itself; the CLI logs a warning and continues.

### Local override

For development and manual testing against a server that is not
behind a configured provider, an environment variable can point
the client at any endpoint directly:

```sh
export ONCE_EVENTS_ENDPOINT=https://events.example.com
once build //some:target --ui
```

The environment variable, when set, takes precedence over the
provider's advertised endpoint for that run only.

## What Once emits today

Not every event in the catalog above is fired yet; the wire
vocabulary is complete but the client fire-points multiply over
time. As of the current release:

- **Run and target lifecycle:** run started, finalizing, completed;
  target queued, started, completed. Fired from the local UI
  publisher.
- **Target phases:** all six phases (queued, cache checking,
  preparing, executing, capturing, publishing) are fired from the
  main graph command around the natural boundaries in the run loop.
- **Log chunks:** every subprocess chunk is forwarded target-scoped
  from the local output observer.
- **Test suites and test cases:** the wire schema is in place, and
  a parser for `cargo test --format=json` output is available in the
  CLI. Wiring the parser into the test command emits per-case events
  end to end; other test frameworks follow the same shape.

The following event kinds are defined in the wire vocabulary but not
fired yet in Once because they need executor internals to grow
richer signals:

- Cache probes and cache miss reasons.
- Artifact publications with the full content reference shape.
- Diagnostic emissions aligned with the lint findings work.
- Target wait reasons for the critical-path view.

Adding a fire-point never requires changing the wire; the transport
already carries every payload the vocabulary defines.

## Compatibility

Version is expressed in the proto package: `once.events.v1`. A future
breaking change lands as `once.events.v2` served alongside `v1` for a
deprecation window. Non-breaking additions land as new payload
variants and are silently tolerated by older clients through the
proto `#[non_exhaustive]` shape.

A server may serve this protocol alongside other gRPC services on the
same endpoint, so there is no requirement for a dedicated hostname,
TLS certificate, or ingress rule.
