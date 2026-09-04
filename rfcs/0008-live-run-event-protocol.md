# Request for Comments 0008: Live Run Event Protocol

## Status

Accepted (v0.21). Open decisions tracked below are non-blocking for the first-slice implementation.

## Motivation

Once has no server-facing event stream. A local axum endpoint publishes a
full run snapshot over Server-Sent Events for whichever local UI is attached,
but nothing reaches the Tuist server, and per-test-case completion is not a
discrete fire-point. Test results land as a normalized JSON blob written
after a whole test target finishes, with a JUnit report attached as an
artifact.

This shape is close to what Bazel's Build Event Protocol produces, and it
inherits the same limitations. A live dashboard cannot render a running
suite because per-case status only exists inside a report file written at
the end. Cache decisions are opaque: BEP reports that an action ran but not
why it missed, which tier served the hit, or how long the transfer took.
Execution phases are opaque: BEP reports queued and completed but not what
happened in between, so slow steps show up as slow targets with no
breakdown.

Tuist should render runs live and it should be materially better than BEP
at what a build dashboard is actually for: showing what happened, why it
happened, and where time went. That requires first-class per-case,
per-target, per-phase, and per-cache-decision events on the wire while the
run is still executing, delivered to a server that folds them into a shared
state model for LiveView.

## Decision

Once will emit a stream of typed events over gRPC to the same endpoint
that serves Bazel Build Event Protocol traffic and, when it lands, remote
execution. Events are defined in a Once-owned proto package,
`once.events.v1`, exposed as a `RunEventService` with a bidirectional
streaming ingest method, a capabilities call, a project-scoped
keyed-hash key retrieval call for argument normalization, and a
fallback acknowledgement probe.
Server-to-client control commands are deferred out of v1.

The server folds events into a shared run-state model that every ingest
service writes into. The projected state is what the dashboard reads;
individual wire vocabularies stay owned by their emitter.

The Bazel Remote Execution protocol is reused as-is for cache, action
cache, execution, and byte-stream traffic. Once contributes ingest, not
replacements.

## Transport

`RunEventService` exposes four methods:

```
service RunEventService {
  rpc GetServerCapabilities(GetServerCapabilitiesRequest) returns (ServerCapabilities);
  rpc GetArgvHashKey(GetArgvHashKeyRequest) returns (ArgvHashKey);
  rpc PublishRunEvents(stream RunEventBatch) returns (stream BatchAck);
  rpc GetRunAck(GetRunAckRequest) returns (RunEventAck);
}
```

`GetArgvHashKey` returns the project-scoped keyed-hash key used by
the client to compute `value_shape_hash` values in `ArgvToken`. See
the Identity and redaction section.

`GetServerCapabilities` is called once before the client mints a run. The
server returns supported protocol versions, maximum batch bytes, maximum
event bytes, maximum log-chunk bytes, maximum unacknowledged events, and
required feature flags. The client picks values within those bounds and
records them on the run so replay is exact.

`PublishRunEvents` is a bidirectional stream. The client streams batches;
the server streams a `BatchAck` for every processed batch. Each batch
carries a client-generated `batch_id` that the corresponding `BatchAck`
echoes for correlation. The ack carries a disposition
(`ACCEPTED`, `REJECTED_STALE`, `REJECTED_INVALID`, `NEEDS_RESYNC`), the
server's current `expected_next_seq`, an optional `retry_after_ms`, and
an optional reduced `max_in_flight_batches` for backpressure.
Acknowledgement advancement of `acked_seq` always means durable
contiguous projection; the server never expresses throttling by
deliberately slowing ack advancement. Transport-level HTTP/2 flow
control remains supplementary.

Continuous acknowledgements are required by the event volume: at peak a
noisy build sustains hundreds to thousands of events per second across
lifecycle, tests, diagnostics, cache, and log chunks. Polling cadence
cannot keep the resend buffer credible for that volume.

`GetRunAck` is a unary read-only probe used on reconnect to discover the
server's durable contiguous acknowledgement for a run before the client
begins resending. It is also safe for out-of-band health checks. It is
never the mid-run acknowledgement path.

Auth reuses the bearer token used by the cache today, carried in a
`tonic` interceptor. The endpoint is shared with the other services on
the server, so there is no new hostname, TLS certificate, or ingress
rule. Tenancy is derived authoritatively from the token; per-event
fields never override it.

## Naming and package layout

Emitter-scoped: `once.events.v1`. Once owns the vocabulary and evolves
it on Once's release cadence. The Tuist server hosts a projector that
maps this vocabulary into the shared run-state model, alongside a
separate projector for Bazel BEP.

Cross-tool query and admin services owned by Tuist itself live under
`tuist.<domain>.v1`. Those describe platform concerns and are not scoped
to a single emitter.

Version lives in the package name. A breaking change becomes
`once.events.v2` on the same server for a deprecation window. `v1` is
never mutated in a wire-incompatible way. `RunStarted.protocol_version`
records the sender's chosen version so replay is exact and mismatches
are logged without breaking the connection.

## Delivery semantics

Each event carries `seq` (monotonic per run, starts at 1) and both a
wall timestamp (`epoch_ms`, Unix milliseconds, for display) and a
monotonic offset (`mono_ns`, elapsed nanoseconds since `run.started`,
authoritative for durations).

A batch carries `run_id`, `batch_id`, an optional list of
`gap_advances`, `seq_from`, and a contiguous sequence of events
starting at `seq_from`. Each `GapAdvance` is a **batch-level control
record**, not an event; none of them consume a sequence number. The
list may be empty (no loss to declare), a single interval (one loss
event), or several intervals (multiple loss events accumulated since
the last successful batch).

**Canonical batch shape (server rejects any other shape with
`REJECTED_INVALID`):**

1. `run_id` and `batch_id` are present.
2. `gap_advances` (possibly empty) is sorted strictly increasing
   and non-overlapping. Formally, for each `i`:
   `gap_advances[i].first_dropped_seq <= gap_advances[i].last_dropped_seq`
   and
   `gap_advances[i].last_dropped_seq < gap_advances[i+1].first_dropped_seq`.
3. If `gap_advances` is non-empty:
   `gap_advances[N-1].last_dropped_seq < seq_from`. Every gap
   strictly precedes the event range. Gaps may not overlap with
   or be interleaved among the batch's events.
4. `events` may be empty (a **control-only batch** that carries
   only loss declarations). If empty, the batch's effect is the
   `gap_advances` application plus the sequence assertion carried
   by `seq_from`; acceptance follows the dedicated control-only
   rule below.
5. If `events` is non-empty, they are strictly contiguous starting
   at `seq_from`.

This canonical shape eliminates ambiguity between duplicate-prefix
acceptance and `REJECTED_STALE`: a sequence number cannot simultaneously
appear in the batch's `gap_advances` and in its `events`, so any
event whose sequence collides with a declared lost interval must
have been declared by a *prior* accepted batch and is therefore
correctly a `REJECTED_STALE` violation of the client contract.

The server applies each `GapAdvance` in order, each subject to the
same four-case decision below. This shape survives both a lost
acknowledgement (the server already accepted a prefix and treats it
as a no-op) and a second overflow while a prior gap batch is still
in flight (the client accumulates a new interval locally and drains
all intervals into the next batch's `gap_advances` list).

Acceptance is checked atomically inside the projector transaction
that also advances `expected_next_seq`, writes the raw events,
updates the projection, and issues the ack.

Each `GapAdvance` in `gap_advances` must satisfy
`first_dropped_seq <= last_dropped_seq`. Its effect on
`expected_next_seq` is one of four cases:

1. `expected_next_seq < first_dropped_seq`: the client is ahead of
   the server (the server has not yet seen events at or before the
   loss interval). Reject the whole batch with `NEEDS_RESYNC`; the
   ack carries the server's actual `expected_next_seq` so the
   client can rebuild.
2. `first_dropped_seq <= expected_next_seq <= last_dropped_seq + 1`:
   the server had already accepted a prefix of the interval (its
   projections are the source of truth for that prefix and are not
   touched). The suffix `[expected_next_seq, last_dropped_seq]` is
   the real loss. Advance `expected_next_seq` to
   `last_dropped_seq + 1`.
3. `expected_next_seq > last_dropped_seq + 1`: the server already
   has everything the client believed lost. Treat the whole
   interval as a no-op; do not touch `expected_next_seq`.
4. In all cases the server logs the observed relationship for
   diagnostics.

The server applies `gap_advances` left-to-right. If any entry falls
into case 1 the entire batch is rejected and no partial advance is
committed. Once `gap_advances` processing is complete, the batch's
events must be strictly contiguous starting at `seq_from` (that is,
the batch is self-consistent).

**Control-only batch (empty `events`).** After `gap_advances`
processing, the server requires `seq_from == expected_next_seq`
(the client's assertion of the next event sequence matches the
server's expectation). If satisfied, accept with `ACCEPTED`; do not
advance `expected_next_seq`; ack carries
`acked_seq = expected_next_seq - 1` (the highest sequence durably
resolved, which after a gap application is the last declared-lost
sequence). If not satisfied, reject with `NEEDS_RESYNC`; ack carries
the server's current `expected_next_seq`.

**Events-present batch.** The disposition is determined by comparing
the declared range `[seq_from, seq_to]` (where
`seq_to = seq_from + len(events) - 1`) against the current
`expected_next_seq` (E):

- `seq_from > E`: the client is ahead of the server (events are
  missing between E and `seq_from`). Reject with `NEEDS_RESYNC`;
  the ack carries E so the client can rebuild.
- `seq_from == E`: normal-path append. Accept with `ACCEPTED`.
  Durably project the events; advance E to `seq_to + 1`; ack
  carries `acked_seq = seq_to` (assuming a fully-contiguous
  durable frontier).
- `seq_from < E` and `seq_to < E`: the whole batch is a duplicate
  prefix (a lost-ack retry, most commonly). Accept with `ACCEPTED`
  idempotently; do not touch the projection; do not advance E; ack
  carries `acked_seq = E - 1`. Per-event dedup rules below still
  apply for divergent-payload detection.
- `seq_from < E <= seq_to`: the batch straddles. Events in
  `[seq_from, E - 1]` are duplicates and go through per-event
  dedup; events in `[E, seq_to]` are new and are durably projected
  in order. Accept with `ACCEPTED`; advance E to `seq_to + 1`; ack
  carries `acked_seq = seq_to`.

Per-event dedup within a duplicate portion:

- `(run_id, seq)` with identical payload is silently accepted (no
  state change).
- `(run_id, seq)` with divergent payload is logged and dropped
  (first-write-wins). No disposition change; the batch is still
  `ACCEPTED`.

`REJECTED_STALE` is reserved for a narrow edge case: the client
attempts to deliver an event at a sequence number that a prior
`GapAdvance` durably declared as lost. This should not occur with a
correct client (loss intervals are recorded before dropped events
leave the ring), but the server enforces it defensively; the ack
carries the current E and the client discards the offending events.

`NEEDS_RESYNC` is the disposition for every sequence-mismatch case
in which the server cannot make forward progress: `seq_from > E`, a
`gap_advances` case 1, or a batch whose events are not
self-contiguous. The client always responds by polling `GetRunAck`
and resuming from `E`.

`BatchAck` carries `run_id`, `batch_id`, disposition, `acked_seq`
(highest contiguous sequence durably **resolved**, meaning either
durably projected as an event or durably declared lost by a prior
`GapAdvance`), `expected_next_seq`, an optional
`observed_high_water_seq`, optional throttle advice, and
`finalization`.

Delivery is at-least-once. The server deduplicates by `(run_id, seq)`.
Duplicates with identical payload are silently accepted; duplicates
with divergent payload are logged and the later is dropped, since
first-write-wins.

The client keeps an in-memory ring buffer of unacknowledged events. On
stream break it reconnects with exponential backoff, calls
`GetRunAck`, and resends from `acked_seq + 1`. The ring reserves
capacity for terminal events that is not consumed by ordinary events,
so overflow can never suppress the run's terminal state. Loss itself
does not need reserved event capacity because it is signalled at
batch level via `gap_advance` and never consumes a sequence number.

Emission never blocks the build for non-terminal events. When
ordinary capacity is full, the oldest unacknowledged non-terminal
events are dropped from the client ring and their sequence numbers
are recorded as a **loss interval** in a locally maintained sorted
set of intervals. This set is data on the client, not events;
because it lives outside the sequence space it does not need to
reserve or collide with any assigned sequence number.

On each `RunEventBatch` send the client atomically drains the
current set into the batch's `gap_advances` list, coalescing any
adjacent or overlapping intervals into a canonical sorted
non-overlapping form. New loss that occurs after that drain but
before acknowledgement is recorded as a fresh interval in the set
and is included in the next batch. There is no ordering constraint
between the in-flight batch and further loss: the in-flight batch
either succeeds (server advances past its declared intervals) or is
rejected (client re-adds its intervals to the set and reissues on
resend).

Late arrivals from the server perspective (events inside a declared
interval that the server has since advanced past) are stale by
construction: `expected_next_seq` has moved on, so any future batch
attempting to deliver them is rejected with `REJECTED_STALE`. The
recovery-from-lost-ack path above handles the converse case where
the server had already processed events the client thought lost.

Terminal delivery uses an explicit intent signal. When the run is
ending, the client emits `run.finalizing` and enters a bounded final
drain (default two seconds) as local behavior. The server transitions
the run to `FINALIZING` on receipt of the intent, and simultaneously
starts a **server-clock finalization grace period** whose duration is
declared in `ServerCapabilities.finalization_grace_ms`. If
`run.completed` arrives before the grace expires, the server
transitions to `FINALIZED`. If the grace expires after finalizing
intent but before `run.completed`, the server transitions to
`FINALIZATION_PENDING`. A run that has received no heartbeats past
its liveness timeout without any finalizing intent transitions to
`RUN_LOST`.

A valid late `run.completed` arriving after `FINALIZATION_PENDING`
transitions the run to `FINALIZED`, provided the run is still inside
the deduplication retention window. Deduplication state remains
retained after this transition until the window ends. Late completion
of a `RUN_LOST` run is also accepted within the retention window and
transitions to `FINALIZED`; the projection preserves the fact that
the run was marked lost so the dashboard can show the recovery.

The client's drain deadline is local behavior only. The server may
record the client's declared drain duration on `run.finalizing` for
diagnostics but never uses it for state-transition timing; a
client-relative monotonic value is not comparable to the server's
clock. `PENDING`, `FINALIZED`, and `LOST` are three distinct states
with three distinct evidence sources; the server never marks
`PENDING` from missing heartbeats alone.

Server-side deduplication state for a `run_id` is retained for a
bounded window after the terminal event (default 24 hours).

**First-contact run creation.** An unknown `run_id` is accepted only
when its first batch starts at `seq_from == 1`, its first event is
`run.started`, and its `gap_advances` list is empty. In that case
the server atomically creates the run and its projection state,
sets `expected_next_seq = 2`, and processes the remainder of the
batch normally. Any other batch shape for an unknown `run_id` is
rejected: a batch with `seq_from != 1`, a first event that is not
`run.started`, or a non-empty `gap_advances` list on first contact
returns `REJECTED_INVALID` with a resolvable error indicating that
the run does not exist. The same disposition applies to a batch for
a `run_id` past its dedup retention window; the client is expected
to log and drop, and to mint a new `run_id` if it still needs to
report.

Batches flush on a short timer (~150 ms) or immediately when a
significant event is emitted (a failed test case, a completed target,
the first byte of standard error). Waiting for the run to finish is
never acceptable for a significant event.

Every event carries either `run_scope`, `target_execution_id`, or
`test_case_execution_id` scoping so log offsets and cache correlations
resolve unambiguously.

## Identity and redaction

Tenancy is derived authoritatively from the bearer token. The wire
never carries a tenant field.

Project or repository identity is supplied in `run.started` as an
opaque project id and is validated against the token's authorized set.
The server rejects a run whose project is not authorized.

`user_id` is derived server-side from the token; the field is not
carried on the wire.

Raw absolute paths, raw environment values, raw hostnames, and
unredacted argument vectors are not part of the v1 schema. They cannot
be sent because there are no fields for them.

The schema exposes only the sanitized shapes:

- `argv_normalized`: an ordered list of `ArgvToken` produced by the
  generic normalization algorithm below.
- `cwd_relative`: a repository-relative path validated for traversal.
  A leading `/`, `..`, or absolute-path shape is rejected at ingest.
- `host_class`: a coarse token such as `darwin-arm64-ci-runner` or
  `linux-amd64-dev`. Never a resolvable hostname.
- `worker_class`: the equivalent for a worker. Never a resolvable
  worker hostname.
- `env_fingerprint`: a stable content-addressed digest over the
  actionable subset of the environment. Never raw env values.

**Argument normalization algorithm (v1).** The client walks the
argument list left-to-right and emits one `ArgvToken` per position:

1. If the token matches the safe literal allowlist (a small,
   explicitly enumerated set of tool and subcommand names such as
   `cargo`, `build`, `test`, `swift`, `xcodebuild`), emit
   `SafeLiteral{value}`. The allowlist is maintained by Once and can
   never contain a value that could carry secrets.
2. If the token matches a boolean flag shape (`--flag`, `-x`), emit
   `FlagKey{key}`.
3. If the token matches the combined form `--key=value`, split and
   emit `NamedValue{key, value_shape_hash}` where `value_shape_hash`
   is a keyed BLAKE3 digest of the raw value under a project-scoped
   secret so identical values cluster in analytics without leaking
   content.
4. Any other token emits `OpaqueValue{value_shape_hash}`. The client
   never infers that an opaque value token belongs to the preceding
   flag key; tool-specific parsing is out of scope for v1.

The algorithm is deliberately conservative. Readable positional
values (package names, target names) show up as `OpaqueValue` unless
the specific literal appears on the allowlist. A future v1.x may
introduce tool-specific parsers behind a per-project setting; the
schema shape for `ArgvToken` does not need to change to add them.

**Worked examples against the v1 allowlist.** All four variants
appear in normal Once and Cargo invocations:

1. `once build //foo:bar` →
   `[SafeLiteral("once"), SafeLiteral("build"), OpaqueValue(hash("//foo:bar"))]`.
   The target name is never on the allowlist; it hashes.
2. `cargo test -p once-core --release` →
   `[SafeLiteral("cargo"), SafeLiteral("test"), FlagKey("-p"), OpaqueValue(hash("once-core")), FlagKey("--release")]`.
   The client does not infer that `once-core` belongs to `-p`; it
   hashes as its own opaque positional. The projector renders this
   run as `cargo test -p ⟨opaque⟩ --release` with the opaque token
   as a stable identifier that clusters across runs with the same
   package value.
3. `cargo build --target=aarch64-apple-darwin` →
   `[SafeLiteral("cargo"), SafeLiteral("build"), NamedValue(key="--target", value_shape_hash=hash("aarch64-apple-darwin"))]`.
   The `--key=value` combined form is the only case where the
   client asserts a value belongs to a key; all other positional
   values hash as `OpaqueValue`.

**Argv hash key lifecycle.** The keyed hash used for
`value_shape_hash` is a project-scoped secret. The client obtains it
by calling `GetArgvHashKey(project_id)` on the same service and
caches it until near expiry. The response returns the raw key
bytes, a non-secret `key_id` for observability and rotation, and an
`expires_at_epoch_ms`. Every `run.started` records the `key_id`
under which its `value_shape_hash` values were computed. The
projector groups values across runs sharing the same `key_id`;
rotation boundaries end a cluster (values re-hashed under a new
key are cryptographically unrelated to values under the old key).

The server rotates keys on a policy cadence and independently on
explicit rotation. A retired `key_id` remains valid on ingest for a
declared grace period (`ArgvHashKey.grace_after_expiry_ms`, default
24 hours). Any run whose `RunStarted.argv_hash_key_id` references a
key still inside its grace window is accepted normally.

A client that receives `KEY_EXPIRED` from the server on
`GetArgvHashKey` or `GetServerCapabilities` refetches and uses the
new key on the next new run. `PublishRunEvents` never fails with
`KEY_EXPIRED` for a run whose `key_id` was valid at `run.started`
time and remains inside its grace window; the server carries the
key material for accepted runs through the retention window.

A run whose `key_id` is past its grace window is still accepted for
ingest (events are not lost), but the server marks the projection
`argv_key_expired_past_grace`. `value_shape_hash` values from that
run are stored but do not cluster with newer runs. This is a
soft-degradation path, never a data-loss path.

A client MUST NOT mint a new `key_id` mid-run: the accumulated
`value_shape_hash` values were computed with the old key, and
switching would break both server acceptance and cross-run
clustering. On explicit rotation the client policy is either to
continue the current run under the old key (accepting the eventual
soft-degradation if grace expires), or to finalize and start a new
run under the new key. Once picks the continue-under-old-key
default; this trades a small amount of end-of-run clustering
fidelity for zero risk of splitting a live run.

Two runs under different `key_id` never cluster together; the
projector shows rotation boundaries explicitly so aggregate metrics
remain honest.

**Safe literal allowlist.** The client and server share a versioned
allowlist of safe literals. `ServerCapabilities.safe_literal_allowlist_version`
declares the server's current version; `RunStarted.safe_literal_allowlist_version`
declares the version the client used. The server validates every
`safe_literal` on ingest against the declared version's allowlist;
values not on the list are quarantined and rewritten to
`opaque_value_hash` in the projection, and a data-quality warning is
recorded on the run.

Version, ownership, and distribution:

- The list is owned by the Once maintainers and lives in the Once
  source tree as a single authoritative file. It is baked into the
  Once client binary at compile time and consumed by the Tuist
  server from the same file, published as a small versioned data
  package. Client and server always read the same source.
- Each release of the list carries a monotonic version string
  `YYYY.MM.DD-vN`. The client and server reject or downgrade a
  mismatch through the projection quarantine path above; they do
  not attempt to synthesize allowlist content from either side.
- The list contains only tokens that are safe to render verbatim
  in dashboards and that cannot plausibly carry secrets: the names
  of build tools and their common subcommands. It never contains
  path fragments, package names, target names, or user identifiers.

The frozen v1 initial version is `2026.09.03-v1` and contains
exactly the following tokens:

- Build/package tools: `cargo`, `rustc`, `clippy`, `rustfmt`,
  `swift`, `swiftc`, `xcodebuild`, `go`, `gofmt`, `npm`, `pnpm`,
  `yarn`, `node`, `tsc`, `python`, `python3`, `pip`, `uv`,
  `ruby`, `bundle`, `mise`, `make`, `ninja`, `cmake`, `bazel`,
  `buck2`, `pants`, `gradle`, `mvn`, `once`.
- Compilers and linkers: `gcc`, `g++`, `clang`, `clang++`, `ld`,
  `lld`, `mold`.
- Common subcommands: `build`, `test`, `run`, `check`, `install`,
  `update`, `lint`, `format`, `fmt`, `bench`, `doc`, `clean`,
  `add`, `remove`, `publish`, `release`, `debug`.
- Frameworks and analyzers with subcommand-like invocations:
  `eslint`, `prettier`, `ruff`, `mypy`, `pyright`, `black`,
  `pytest`, `jest`, `vitest`, `rspec`, `phpunit`.

Additions to the list follow the normal Once release cycle and
must not include anything that could carry variable content. A
future safe-literal addition of, say, `xcresulttool` is a schema
non-event: existing runs recorded under older versions continue to
render correctly because the version stamp on each run pins the
list that produced its `safe_literal` values.

Log content is inherently arbitrary text and is off by default. A
project must explicitly opt in to log ingestion and the projection
records the opt-in state on every run so retention and access controls
can honour it.

Diagnostic messages, test names, parameters, tags, target ids, and
artifact paths remain author-controlled but are subject to server-side
policy validation. Suspicious content (absolute paths, secrets-shape
tokens) is quarantined and surfaced as a data-quality warning on the
projected state rather than silently dropped.

**Path fields, general rule.** Every field on any event that carries
a source path is workspace-relative and is validated for traversal
at ingest. This applies to `TestCaseStarted.file`,
`Location.file` (in `DiagnosticEmitted.primary` and `related`),
`ArtifactPublished.workspace_relative_path`,
`CacheMissReason.differing_inputs`, and any similar future field. A
value with a leading `/`, a `..` segment, an absolute-path shape
(including drive letters on Windows), or a URI scheme is rejected as
a data-quality violation; the enclosing event is projected with the
offending field cleared and a warning recorded on the run. The rule
is enforced on the server; the client is expected to emit only
workspace-relative paths.

## Execution and attempt identity

Two levels of identity:

- `target_instance_id`: the logical target occurrence within this
  run. Two calls to the same `target_id` under different configurations
  or graph instances get different `target_instance_id`. The projector
  uses it as the stable roll-up key. Minted at graph compile time.
- `target_execution_id`: one attempt of one target instance. Minted at
  `target.queued`. Reused across `started`, phase transitions,
  `completed`, `cancelled`, cache probes, log chunks, and artifact
  publications for that attempt.

Retries produce a new `target_execution_id`; the previous one is
referenced from a `target.retried` link event that fires between the
old attempt's `target.completed` (or `target.cancelled`) and the new
attempt's `target.queued`. `target.queued` remains the authoritative
mint for `target_execution_id`; `target.retried` never mints, it only
links.

Test cases follow the same rule. `test_case.started` mints
`test_case_execution_id`. A retry emits `test_case.retried` (linking
old to new) between the old attempt's `test_case.completed` and the
new attempt's `test_case.started`. A stable `case_id` within a
`target_execution_id` survives parameterization and display-name
collisions; the human-visible name, class, file, and parameters live
on `test_case.started`.

## Content references

Every place the protocol references CAS-backed bytes uses a single
`ContentRef` message with a hash algorithm, digest, size, optional
content namespace, and optional media type. Bare digest strings are
never used.

## Cache observability

Cache is a first-class concern. This is the largest place the protocol
goes beyond BEP.

Every cache interaction for one lookup is tied together by a
**`cache_decision_id`**, minted by the client at the first probe of
that lookup. Every subsequent probe, miss reason, download, upload, and
store-reuse event for the same lookup carries the same
`cache_decision_id`. The projector groups them into one cache decision
row per target execution: "attempted local, missed in 4 ms; attempted
remote, hit in 380 ms; downloaded 2.1 MiB in 210 ms; miss reason
n/a." Without this identifier a projector cannot safely group
concurrent lookups within a single target execution.

A single action lookup that misses the local tier and hits the remote
tier produces two `cache.probe` events with ordered `tier_index` and
per-tier outcomes; hybrid is derived from the sequence rather than a
single blurry enum. Every upload and every download is its own event
with tier, size, duration, transfer bytes, and target execution
correlation.

When a target misses cache and executes, the client emits
`cache.miss_reason` carrying:

- `primary_reason`: the dominant kind from `MissReasonKind`.
- `all_reasons`: every detected kind (some misses have several).
- `analysis_status`: `COMPLETE`, `PARTIAL`, `TRUNCATED`, or
  `UNAVAILABLE`.
- `differing_inputs`: workspace-relative paths that changed, capped;
  paired with `differing_inputs_total_count` and
  `differing_inputs_truncated`.
- `baseline_resolution`: a required oneof, never absent:
  `REFERENCE{previous_run_id, previous_target_instance_id,
  previous_target_execution_id, previous_action_digest,
  baseline_selection_reason}` when a comparison baseline was found;
  `NONE_AVAILABLE` when the project has no prior successful run for
  this target instance; `NOT_ATTEMPTED` when baseline lookup was
  disabled or skipped for cost reasons; `UNAVAILABLE` when the lookup
  was attempted but failed (transient error, missing metadata, etc.).

Baseline resolution is always present so the projector can render a
reliable state ("no prior run to compare against," "lookup skipped,"
"lookup failed," or the actual baseline drill-down) without doing an
expensive lookup merely to prove absence.

`cache.store_reused` records an upload that was skipped because the
digest was already present at the tier. Its `bytes_saved` is defined
as the avoided transfer bytes for that operation, which may be less
than `ContentRef.size_bytes` when partial transfer was involved. The
projector deduplicates reuse across retries so the "bytes saved"
dashboard metric is not inflated.

## Scheduler blocking

While a target is in `QUEUED`, the client optionally emits `target.wait`
events describing what the target is waiting on: `DEPENDENCY_WAIT`,
`RESOURCE_WAIT`, `WORKER_WAIT`, `THROTTLED`, or `INFRASTRUCTURE`. When
the reason is `DEPENDENCY_WAIT`, the event references the specific
`target_execution_id` that is blocking. Emitted on transition and on
timer while the queued time exceeds a threshold.

This is what turns per-phase durations into a real critical-path view.
"This target waited 12 seconds for its transitive dependency to
finish" is a story per-phase timing cannot tell alone.

## Target phases

Time inside a target is exposed as phase transitions so slow steps
have names. Every phase transition is a `target.phase` event carrying
the target execution and the new phase. There is no `DONE` phase;
`target.completed` is the terminal signal.

Phases:

- `QUEUED`: scheduler and dependency wait. `target.wait` may fire
  during this phase.
- `CACHE_CHECKING`: action-cache lookup and decision. Cache events
  under one `cache_decision_id` fall inside this phase.
- `PREPARING`: worker allocation, sandbox setup, tool warmup, input
  download and materialization.
- `EXECUTING`: subprocess running.
- `CAPTURING`: output collection and validation.
- `PUBLISHING`: cache and artifact uploads.

**Phase exclusivity invariant (v1).** Phases are mutually exclusive
logical phases per target execution: at most one `target.phase` is
current at any moment, and transitions are strictly ordered along the
list above. In particular, cache lookup must complete before
`PREPARING` begins even on backends where the operations could
overlap. Cache events retain finer-grained timing under
`cache_decision_id`, so overlapping cache and materialization work is
visible in the cache-decision detail without breaking the phase
timeline the dashboard renders as a single bar. A future revision may
introduce overlapping activity spans if needed; v1 keeps the model
simple.

The projector derives per-phase durations by subtracting successive
monotonic timestamps. LiveView renders a phase bar so a slow upload is
visually distinct from a slow execution.

## Event catalog (v1)

Every event is a variant of the `RunEvent` payload oneof. Field lists
here are the intent; the appendix is the source of truth.

Run lifecycle:

- `run.started` records Once version, protocol version, host class,
  git revision, argv_normalized, cwd_relative, env_fingerprint, root
  graph digest, project id, and the effective limits chosen at
  capability negotiation.
- `run.finalizing` marks the client's intent to finalize the run.
  Emitted immediately before the final drain begins.
- `run.completed` records terminal state as a result enum, optional
  cancellation reason, wall duration, and totals.
- `run.heartbeat` fires every five seconds while the run is active.
- Loss is signalled at batch level via `RunEventBatch.gap_advance`,
  not as an event, so it never consumes a sequence number or
  collides with retained unacknowledged events. See Delivery
  semantics.

Graph analysis:

- `target.instance` records a target instance with its `target_id`
  and configuration digest at graph compile time. Emitted for each
  distinct target instance in the compiled graph.
- `graph.compiled` records the graph digest, target count, target-kind
  histogram, and the requested roots.

**Graph ordering invariant (v1).** Every `target.instance` for the
run must be emitted strictly before `graph.compiled`, and
`graph.compiled.target_count` must equal the number of
`target.instance` events preceding it. The server enforces the exact
count when the sequence range spanning graph analysis has no declared
gap. When a gap falls in that range, the server records
`graph_instance_completeness = DEGRADED` on the projection instead of
rejecting the run.

Target execution:

- `target.queued` mints `target_execution_id` and references
  `target_instance_id`. Carries kind, capability, `ContentRef` action
  digest, `ContentRef` input digest, dep target executions, and
  attempt number.
- `target.started`, `target.phase`, `target.wait`, `target.completed`,
  `target.cancelled`, `target.retried` describe the attempt lifecycle
  as documented above.

Tests:

- `test_suite.started`, `test_suite.completed`.
- `test_case.started`, `test_case.completed`, `test_case.retried`.

Logs:

- `log.chunk`, `log.truncated`. Scoped by `LogScope` oneof over run,
  target execution, or test case execution. Chunks in a scope never
  overlap.

Cache and artifacts:

- `cache.probe`, `cache.miss_reason`, `cache.upload`, `cache.download`,
  `cache.store_reused`. All carry `cache_decision_id`.
- `artifact.published`.

Diagnostics:

- `diagnostic.emitted` with tool, code, primary and related
  `Location`s including end line and column, stable fingerprint for
  dedup, size-capped message, and `ContentRef` snippet.

Server-to-client control commands are omitted from v1.

## Projector split on the server

The server hosts two ingest services, one per emitter, feeding the
same projected state:

- `google.devtools.build.v1.PublishBuildEvent` for Bazel and any
  other BEP producer.
- `once.events.v1.RunEventService` for Once and future
  Once-compatible emitters.

Each service ships with its own projector module. Both projectors
write into the same tables (runs, run targets, run target instances,
run target executions, run test cases, run test case executions, run
logs, run cache decisions, run cache operations, run artifacts).
LiveView subscribes to the projected state, never to the raw event
stream. Bazel runs render at target granularity with a JUnit link;
Once runs render with per-case tree updates, per-phase bars, and
per-cache-decision detail in real time.

Raw events are retained as an observability and replay feature, not
as the transactional source of truth. The transactional source of
truth is the projected state, which is written atomically with ack
advancement per batch. Raw events stream into a time-series store off
the ingest transaction so a failed archive never blocks or reorders
ack. When raw retention is enabled the dashboard offers replay; when
disabled it offers projection only.

## Bringing this online

1. Introduce a typed event bus in the Once core that wraps today's
   output observer and publisher without changing user-visible
   behaviour. The local UI becomes the first subscriber.
2. Add per-test-case, per-phase, and per-cache-decision fire-points
   in the runner and executor for at least one framework and one
   target kind as a proof of concept.
3. Add an events client that speaks the gRPC service. Bidirectional
   stream with per-batch ack, ring buffer with reserved terminal
   capacity (loss is signalled at batch level via `gap_advance` and
   does not need reserved event capacity), resume via `GetRunAck` on
   reconnect, bounded final drain with `run.finalizing` intent.
   Behind an opt-in configuration key.
4. Land the ingest service on the Tuist side, the projectors, and
   the LiveView dashboard.

## Open decisions

1. `cache.miss_reason` baseline selection. "Last successful run on
   this branch" is the most defensible default; alternatives
   ("last successful run this project has seen," "last run on the
   same commit") should be enumerable rather than hard-coded.
2. `target.wait` cadence and threshold. Emitting on every state
   change plus a timer beyond N seconds is the current proposal.
   Cheaper alternatives (only on transition, only on a coarse
   timer) trade richness for volume.
3. Server retention windows for dedup state (24 hours proposed) and
   raw events (per-project) validated against real cost.
4. Common content-addressable artifact reference format. `ContentRef`
   as proposed here needs reconciling with the server's existing CAS
   conventions.

## Appendix: strawman proto

```proto
syntax = "proto3";
package once.events.v1;

service RunEventService {
  rpc GetServerCapabilities(GetServerCapabilitiesRequest) returns (ServerCapabilities);
  rpc GetArgvHashKey(GetArgvHashKeyRequest) returns (ArgvHashKey);
  rpc PublishRunEvents(stream RunEventBatch) returns (stream BatchAck);
  rpc GetRunAck(GetRunAckRequest) returns (RunEventAck);
}

message GetServerCapabilitiesRequest {}
message GetRunAckRequest { string run_id = 1; }
message GetArgvHashKeyRequest { string project_id = 1; }

message ServerCapabilities {
  repeated string supported_protocol_versions = 1;   // e.g. ["1.0"]
  uint32 max_batch_bytes = 2;
  uint32 max_event_bytes = 3;
  uint32 max_unacked_events = 4;
  uint32 max_log_chunk_bytes = 5;
  repeated string required_features = 6;
  bool log_ingestion_available = 7;
  bool raw_event_retention_available = 8;
  uint32 finalization_grace_ms = 9;
  uint32 dedup_retention_seconds = 10;
  string safe_literal_allowlist_version = 11;
}

message ArgvHashKey {
  string key_id = 1;                 // non-secret identifier
  bytes key_bytes = 2;               // 32-byte BLAKE3 key
  int64 expires_at_epoch_ms = 3;
  uint32 grace_after_expiry_ms = 4;  // retired key remains valid on ingest for this long
}

message RunEventBatch {
  string run_id = 1;
  string batch_id = 2;                   // client-generated, echoed in BatchAck
  repeated GapAdvance gap_advances = 3;  // batch-level control records; sorted, non-overlapping
  uint64 seq_from = 4;
  repeated RunEvent events = 5;
}

message GapAdvance {
  uint64 first_dropped_seq = 1;
  uint64 last_dropped_seq = 2;
  string reason = 3;                     // e.g. "buffer_overflow"
}

message BatchAck {
  string run_id = 1;
  string batch_id = 2;
  AckDisposition disposition = 3;
  uint64 acked_seq = 4;                  // highest contiguous durably resolved (projected or declared lost)
  uint64 expected_next_seq = 5;
  uint64 observed_high_water_seq = 6;    // optional
  uint32 retry_after_ms = 7;             // 0 = no advice
  uint32 max_in_flight_batches = 8;      // 0 = no change
  RunFinalization finalization = 9;
}

// GetRunAck returns the same shape as BatchAck without echoing a batch_id.
// The old RunEventAck name is dropped; PublishRunEvents streams BatchAck.
message RunEventAck {
  string run_id = 1;
  uint64 acked_seq = 2;                  // highest contiguous durably resolved
  uint64 expected_next_seq = 3;
  uint64 observed_high_water_seq = 4;
  RunFinalization finalization = 5;
}

enum AckDisposition {
  ACK_DISPOSITION_UNSPECIFIED = 0;
  ACK_DISPOSITION_ACCEPTED = 1;
  ACK_DISPOSITION_REJECTED_STALE = 2;    // batch overlaps already-acked seqs
  ACK_DISPOSITION_REJECTED_INVALID = 3;  // shape/invariant violation
  ACK_DISPOSITION_NEEDS_RESYNC = 4;      // client should re-poll GetRunAck
}

enum RunFinalization {
  RUN_FINALIZATION_UNSPECIFIED = 0;
  RUN_FINALIZATION_ACTIVE = 1;           // running, no finalizing intent seen
  RUN_FINALIZATION_FINALIZING = 2;       // intent received, awaiting completed
  RUN_FINALIZATION_FINALIZED = 3;        // completed received
  RUN_FINALIZATION_FINALIZATION_PENDING = 4;  // intent received but deadline elapsed
  RUN_FINALIZATION_LOST = 5;             // no intent, liveness timeout expired
}

message ContentRef {
  HashAlgorithm hash_algorithm = 1;
  bytes digest = 2;
  uint64 size_bytes = 3;
  string namespace = 4;     // optional, e.g. "once.evidence.v1"
  string media_type = 5;    // optional, e.g. "application/xml"
}

enum HashAlgorithm {
  HASH_ALGORITHM_UNSPECIFIED = 0;
  HASH_ALGORITHM_BLAKE3 = 1;
  HASH_ALGORITHM_SHA256 = 2;
}

message RunEvent {
  uint64 seq = 1;
  int64 epoch_ms = 2;      // wall clock, for display
  int64 mono_ns = 3;       // elapsed since run.started

  oneof payload {
    // Lifecycle
    RunStarted         run_started         = 10;
    RunFinalizing      run_finalizing      = 11;
    RunCompleted       run_completed       = 12;
    RunHeartbeat       run_heartbeat       = 13;
    // gap.declared removed; loss is signalled at batch level via RunEventBatch.gap_advance.

    // Graph
    GraphCompiled      graph_compiled      = 20;
    TargetInstance     target_instance     = 21;

    // Target execution
    TargetQueued       target_queued       = 30;
    TargetStarted      target_started      = 31;
    TargetPhase        target_phase        = 32;
    TargetWait         target_wait         = 33;
    TargetCompleted    target_completed    = 34;
    TargetCancelled    target_cancelled    = 35;
    TargetRetried      target_retried      = 36;

    // Tests
    TestSuiteStarted   test_suite_started  = 40;
    TestSuiteCompleted test_suite_completed = 41;
    TestCaseStarted    test_case_started   = 42;
    TestCaseCompleted  test_case_completed = 43;
    TestCaseRetried    test_case_retried   = 44;

    // Logs
    LogChunk           log_chunk           = 50;
    LogTruncated       log_truncated       = 51;

    // Cache
    CacheProbe         cache_probe         = 60;
    CacheMissReason    cache_miss_reason   = 61;
    CacheUpload        cache_upload        = 62;
    CacheDownload      cache_download      = 63;
    CacheStoreReused   cache_store_reused  = 64;

    // Artifacts and diagnostics
    ArtifactPublished  artifact_published  = 70;
    DiagnosticEmitted  diagnostic_emitted  = 71;
  }
}

// ------------ Lifecycle ------------

message RunStarted {
  string once_version = 1;
  string protocol_version = 2;
  string host_class = 3;
  string git_rev = 4;
  bool git_dirty = 5;
  repeated ArgvToken argv_normalized = 6;
  string argv_hash_key_id = 7;          // key under which value_shape_hash was computed
  string safe_literal_allowlist_version = 8;  // version validated against on ingest
  string cwd_relative = 9;
  string env_fingerprint = 10;
  ContentRef root_graph_digest = 11;
  string project_id = 12;
  EffectiveLimits effective_limits = 13;
}

message ArgvToken {
  oneof token {
    string safe_literal = 1;       // matches Once's safe-literal allowlist
    string flag_key = 2;           // e.g. "--release" or "-x"
    NamedValue named_value = 3;    // "--key=value" combined form only
    string opaque_value_hash = 4;  // any other token; hashed opaquely
  }
}

message NamedValue {
  string key = 1;                  // the "--key" side of "--key=value"
  string value_shape_hash = 2;     // keyed BLAKE3 of the value
}

message EffectiveLimits {
  uint32 max_batch_bytes = 1;
  uint32 max_event_bytes = 2;
  uint32 max_unacked_events = 3;
  uint32 max_log_chunk_bytes = 4;
  bool log_ingestion_enabled = 5;
  bool raw_event_retention_enabled = 6;
}

message RunFinalizing {
  // Client's local drain deadline, recorded for diagnostics only.
  // The server never uses this for state-transition timing; it starts
  // its own server-clock grace period on receipt.
  int64 declared_drain_ms = 1;
}

message RunCompleted {
  RunResult result = 1;
  string cancellation_reason = 2;
  int64 wall_ms = 3;
  RunTotals totals = 4;
}

enum RunResult {
  RUN_RESULT_UNSPECIFIED = 0;
  RUN_RESULT_SUCCEEDED = 1;
  RUN_RESULT_FAILED = 2;
  RUN_RESULT_CANCELLED = 3;
  RUN_RESULT_TIMED_OUT = 4;
  RUN_RESULT_INFRASTRUCTURE_ERROR = 5;
}

message RunTotals {
  map<string, uint32> targets_by_result = 1;
  map<string, uint32> cases_by_result = 2;
  double cache_hit_rate = 3;
  uint64 cache_bytes_downloaded = 4;
  uint64 cache_bytes_uploaded = 5;
  uint64 cache_bytes_saved = 6;
}

message RunHeartbeat {}

// GapDeclared removed. Loss is signalled at batch level via
// RunEventBatch.gap_advance so it does not consume an event sequence
// number and cannot collide with retained unacknowledged events.

// ------------ Graph ------------

message GraphCompiled {
  ContentRef graph_digest = 1;
  uint32 target_count = 2;
  map<string, uint32> kind_histogram = 3;
  repeated string roots = 4;
}

message TargetInstance {
  string target_instance_id = 1;
  string target_id = 2;                    // opaque graph label
  ContentRef configuration_digest = 3;
}

// ------------ Target execution ------------

message TargetQueued {
  string target_execution_id = 1;
  string target_instance_id = 2;
  string kind = 3;
  string capability = 4;
  ContentRef action_digest = 5;
  ContentRef input_digest = 6;
  repeated string dep_target_executions = 7;
  uint32 attempt = 8;
}

message TargetStarted {
  string target_execution_id = 1;
  string worker_class = 2;                 // opaque, no hostname
}

message TargetPhase {
  string target_execution_id = 1;
  Phase phase = 2;
}

enum Phase {
  PHASE_UNSPECIFIED = 0;
  PHASE_QUEUED = 1;
  PHASE_CACHE_CHECKING = 2;
  PHASE_PREPARING = 3;
  PHASE_EXECUTING = 4;
  PHASE_CAPTURING = 5;
  PHASE_PUBLISHING = 6;
}

message TargetWait {
  string target_execution_id = 1;
  WaitReason reason = 2;
  string blocking_target_execution_id = 3; // set when DEPENDENCY_WAIT
  string resource_kind = 4;                // set when RESOURCE_WAIT
}

enum WaitReason {
  WAIT_REASON_UNSPECIFIED = 0;
  WAIT_REASON_DEPENDENCY_WAIT = 1;
  WAIT_REASON_RESOURCE_WAIT = 2;
  WAIT_REASON_WORKER_WAIT = 3;
  WAIT_REASON_THROTTLED = 4;
  WAIT_REASON_INFRASTRUCTURE = 5;
}

message TargetCompleted {
  string target_execution_id = 1;
  TargetResult result = 2;
  bool was_cached = 3;
  optional int32 exit_code = 4;
  ContentRef evidence_digest = 5;
  int64 duration_ms = 6;
}

enum TargetResult {
  TARGET_RESULT_UNSPECIFIED = 0;
  TARGET_RESULT_SUCCEEDED = 1;
  TARGET_RESULT_FAILED = 2;
  TARGET_RESULT_SKIPPED = 3;
  TARGET_RESULT_CANCELLED = 4;
  TARGET_RESULT_TIMED_OUT = 5;
  TARGET_RESULT_INFRASTRUCTURE_ERROR = 6;
}

message TargetCancelled {
  string target_execution_id = 1;
  string reason = 2;
}

message TargetRetried {
  string previous_target_execution_id = 1;
  string new_target_execution_id = 2;      // minted by the following target.queued
  uint32 new_attempt = 3;
  string reason = 4;
}

// ------------ Tests ------------

message TestSuiteStarted {
  string target_execution_id = 1;
  string suite_id = 2;
  optional uint32 planned_case_count = 3;
}

message TestSuiteCompleted {
  string target_execution_id = 1;
  string suite_id = 2;
  TestTotals totals = 3;
  ContentRef junit_digest = 4;
}

message TestTotals {
  uint32 passed = 1;
  uint32 failed = 2;
  uint32 skipped = 3;
  uint32 errored = 4;
  uint32 timed_out = 5;
  uint32 cancelled = 6;
  uint32 flaky_final_pass = 7;
  uint32 flaky_final_fail = 8;
}

message TestCaseStarted {
  string test_case_execution_id = 1;
  string target_execution_id = 2;
  string case_id = 3;
  string name = 4;
  string class_name = 5;
  string file = 6;                 // workspace-relative; validated for traversal at ingest
  string parameters = 7;
  repeated string tags = 8;
  uint32 attempt = 9;
}

message TestCaseCompleted {
  string test_case_execution_id = 1;
  TestCaseResult result = 2;
  bool was_flaky = 3;
  int64 duration_ms = 4;
  TestFailure failure = 5;
}

enum TestCaseResult {
  TEST_CASE_RESULT_UNSPECIFIED = 0;
  TEST_CASE_RESULT_PASSED = 1;
  TEST_CASE_RESULT_FAILED = 2;
  TEST_CASE_RESULT_SKIPPED = 3;
  TEST_CASE_RESULT_TIMED_OUT = 4;
  TEST_CASE_RESULT_ERRORED = 5;
  TEST_CASE_RESULT_CANCELLED = 6;
}

message TestCaseRetried {
  string previous_test_case_execution_id = 1;
  string new_test_case_execution_id = 2;
  uint32 new_attempt = 3;
  string reason = 4;
}

message TestFailure {
  string message = 1;
  string expected = 2;
  string actual = 3;
  ContentRef stack_digest = 4;
}

// ------------ Logs ------------

message LogChunk {
  LogScope scope = 1;
  Stream stream = 2;
  int64 offset = 3;              // per (scope, stream)
  bytes bytes = 4;
}

message LogTruncated {
  LogScope scope = 1;
  Stream stream = 2;
  int64 bytes_dropped = 3;
  int64 since_offset = 4;
}

message LogScope {
  oneof scope {
    RunScope run = 1;
    string target_execution_id = 2;
    string test_case_execution_id = 3;
  }
}

message RunScope {}

enum Stream {
  STREAM_UNSPECIFIED = 0;
  STREAM_STDOUT = 1;
  STREAM_STDERR = 2;
}

// ------------ Cache ------------

message CacheProbe {
  string cache_decision_id = 1;
  string target_execution_id = 2;
  ContentRef action_digest = 3;
  string tier = 4;                 // e.g. "local", "remote", "peer"
  uint32 tier_index = 5;           // ordinal within this decision
  CacheOutcome outcome = 6;
  int64 duration_ms = 7;
  string error_class = 8;          // set when outcome == ERROR
}

enum CacheOutcome {
  CACHE_OUTCOME_UNSPECIFIED = 0;
  CACHE_OUTCOME_HIT = 1;
  CACHE_OUTCOME_MISS = 2;
  CACHE_OUTCOME_ERROR = 3;
  CACHE_OUTCOME_BYPASSED = 4;
}

message CacheMissReason {
  string cache_decision_id = 1;
  string target_execution_id = 2;
  MissReasonKind primary_reason = 3;
  repeated MissReasonKind all_reasons = 4;
  MissAnalysisStatus analysis_status = 5;
  repeated string differing_inputs = 6;    // capped
  uint32 differing_inputs_total_count = 7;
  bool differing_inputs_truncated = 8;
  BaselineResolution baseline_resolution = 9;   // required, never absent
}

enum MissReasonKind {
  MISS_REASON_KIND_UNSPECIFIED = 0;
  MISS_REASON_KIND_FIRST_SEEN = 1;
  MISS_REASON_KIND_INPUTS_CHANGED = 2;
  MISS_REASON_KIND_COMMAND_CHANGED = 3;
  MISS_REASON_KIND_ENV_CHANGED = 4;
  MISS_REASON_KIND_TOOL_CHANGED = 5;
  MISS_REASON_KIND_SALT_CHANGED = 6;
  MISS_REASON_KIND_UNKNOWN = 7;
}

enum MissAnalysisStatus {
  MISS_ANALYSIS_STATUS_UNSPECIFIED = 0;
  MISS_ANALYSIS_STATUS_COMPLETE = 1;
  MISS_ANALYSIS_STATUS_PARTIAL = 2;
  MISS_ANALYSIS_STATUS_TRUNCATED = 3;
  MISS_ANALYSIS_STATUS_UNAVAILABLE = 4;
}

message BaselineResolution {
  oneof kind {
    BaselineReference reference = 1;
    BaselineNoneAvailable none_available = 2;
    BaselineNotAttempted not_attempted = 3;
    BaselineUnavailable unavailable = 4;
  }
}

message BaselineReference {
  string previous_run_id = 1;
  string previous_target_instance_id = 2;
  string previous_target_execution_id = 3;
  ContentRef previous_action_digest = 4;
  string selection_reason = 5;      // e.g. "last_successful_on_branch"
}

message BaselineNoneAvailable {}    // project has no prior run for this target instance
message BaselineNotAttempted {      // lookup skipped
  string skip_reason = 1;           // e.g. "disabled_by_config", "cost_budget_exceeded"
}
message BaselineUnavailable {       // lookup attempted, failed
  string failure_reason = 1;        // e.g. "transient_error", "missing_metadata"
}

message CacheUpload {
  string cache_decision_id = 1;
  string target_execution_id = 2;
  ContentRef content = 3;
  string tier = 4;
  string kind = 5;                 // "evidence", "junit", "output", "user"
  int64 duration_ms = 6;
  uint64 bytes_transferred = 7;
}

message CacheDownload {
  string cache_decision_id = 1;
  string target_execution_id = 2;
  ContentRef content = 3;
  string tier = 4;
  string kind = 5;
  int64 duration_ms = 6;
  uint64 bytes_transferred = 7;
}

message CacheStoreReused {
  string cache_decision_id = 1;
  string target_execution_id = 2;
  ContentRef content = 3;
  string tier = 4;
  string kind = 5;
  uint64 bytes_saved = 6;          // avoided transfer bytes for this op
}

// ------------ Artifacts and diagnostics ------------

message ArtifactPublished {
  string target_execution_id = 1;
  string kind = 2;
  ContentRef content = 3;
  string workspace_relative_path = 4;
}

message DiagnosticEmitted {
  string target_execution_id = 1;
  Severity severity = 2;
  string tool = 3;
  string code = 4;
  string message = 5;              // size-capped
  Location primary = 6;
  repeated Location related = 7;
  string fingerprint = 8;          // stable across runs for dedup
  ContentRef snippet = 9;
}

message Location {
  string file = 1;                 // workspace-relative
  uint32 start_line = 2;
  uint32 start_column = 3;
  uint32 end_line = 4;
  uint32 end_column = 5;
}

enum Severity {
  SEVERITY_UNSPECIFIED = 0;
  SEVERITY_NOTE = 1;
  SEVERITY_WARNING = 2;
  SEVERITY_ERROR = 3;
}
```
