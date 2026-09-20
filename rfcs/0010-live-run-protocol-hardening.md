# Live run protocol design review

## Decision

Keep the typed event stream, contiguous durable acknowledgement, explicit loss
intervals, and separation between event ingestion and remote execution. They
are a useful foundation. Do not freeze the current implementation as a complete
distributed build observability contract yet.

An event protocol can make a build explainable, but it cannot establish build
correctness. Dependency declarations, reproducible action keys, hermetic
execution, resource admission, and output validation remain the executor's
responsibility. Ingest failure must not change a build's result. Observability
must distinguish unknown, lost, reused, and executed work without manufacturing
measurements to make a dashboard look complete.

This review covers the new action and host sample records, their producers,
retained target outcomes, and the delivery path they depend on. The production
server and its projector are outside this repository; its conformance is not
established by the client tests.

## Corrections implemented

- Keep `run.started` in a separate slot until acknowledged. Ordinary overflow
  must not make an unknown run impossible to create. Samples produced during
  preflight are excluded from the run stream.
- Keep loss intervals until their sequences are durably resolved. Sending a
  batch is not evidence of acceptance. Partial acknowledgement trims an
  interval, and reconnect preserves its unresolved suffix.
- Ignore events after the terminal event without allocating sequence numbers
  or declaring imaginary loss. A repeated terminal does not replace the first.
- Use one delivery loop for normal runs, shutdown, and reconnect. Shutdown
  survives reconnect; its deadline covers stream establishment, acknowledgement,
  probes, and retry delays. Expiry is an explicit delivery error.
- Permit one outstanding batch per stream and validate its correlation before
  accepting an acknowledgement. Honor retry delay advice. This deliberately
  conservative window prevents duplicate floods; expanding it requires measured
  throughput evidence and an explicit bounded ledger of outstanding batches.
- Subscribe before returning a bus-backed delivery future. Run publication
  cannot race the first poll of that future.
- Separate target phase completion from declared action completion on the wire.
  Phase names must not be magic capabilities that alter action counts.
- Reused targets retain declared action identities, but report zero current
  execution duration, no fabricated worker, and the retained individual action
  key instead of the aggregate target key. Reuse does not report a download unless bytes moved.
- Connect the action runner and live reporter to the application. Publish each
  action's completion when its future finishes, including failures, before
  aggregating the target outcome.
- Negotiate protocol version and wire limits before publication. Bound event,
  batch, log fragment, and unacknowledged ordinary-event bytes; declare
  oversized records as sequence loss. Track loss before sequence assignment
  separately from sequence gaps.
- Split output into bounded fragments with monotonically increasing offsets
  for each target and stream. Emit periodic heartbeats and reserve protected
  finalizing and completed events.
- Default argument reporting to strict redaction. Exposing workspace names
  requires an explicit `argv_privacy = "workspace"` setting.
- Generate the website's field, service, and variant reference directly from
  the versioned wire definition during the Elixir documentation build.
- Report an action attempt's start and completion with a run-local tuple.
  The logical action selects attempt one for current-run execution, or zero
  when it is reused without a new attempt. Resource samples identify their
  run-local host scope and actual sampling interval.
- Preserve unknown test-case outcomes rather than turning them into passes.
  Retrospective results identify their case, suite, and attempt without
  fabricating a start event or a duration.

## Dashboard links

`BatchAck.dashboard_url` and `RunEventAck.dashboard_url` carry the canonical
absolute web link for an authorized run. A server returns it only after durable
run creation. The route is server-owned, opaque to the client, and must not
contain bearer credentials or a capability that bypasses dashboard access
control. Browsing it uses the dashboard's normal authentication.

Once invokes its link handler once after validating an acknowledgement that
resolves sequence 1. The command prints the link while the build continues.
The reconnect probe can recover the link if the first acknowledgement was lost.
A link is optional and its absence never prevents ingestion.

Relative links, non-web schemes, embedded credentials, whitespace, and
terminal controls are rejected. Clients never guess server routes.

The discovery document at `/.well-known/once` has this minimal shape:

```json
{
  "events": ["grpcs://build.example.com"]
}
```

An absent document disables reporting. Fetching the
public discovery document has a time and size limit and sends no bearer token.
The selected event service receives authenticated calls. Endpoint trust must
follow the configured provider's trust boundary.

## Identity and causality

Within a run, a declared action outcome is identified by
`(target_execution_id, capability, action_index)`. The index is declaration
order after deterministic expansion, not completion order. The optional
identifier is a display label. A retry of a configured target must receive a
new target execution identity. A cache key is content identity, never execution
identity: two executions may have the same key.

The wire now has a separate action-attempt identity and a selected result.
Current execution emits one attempt, while retained target reuse selects zero.
Before adding retries or speculative local and remote execution, make retry,
cancellation, resource wait, cache lookup, and output publication reference the
attempt they describe. One logical action can then have several attempts, but
exactly one selected result. Do not retrofit those relationships onto
`worker_id`, a timestamp, or an action key.

The current attempt starts show running actions, but cannot calculate a reliable
critical path or distinguish an executor lost during an attempt from an
unfinished action. A complete lifecycle still needs declared action
dependencies, queue events, and explicit attempt cancellation and timeout.
Target summaries must remain aggregates of declared actions, with a separate
measure of whether the stream was complete enough to derive those aggregates.

## Time and resources

Sequence numbers order the single reporter's stream. Wall clocks are for
human correlation, not ordering or elapsed durations. Use producer monotonic
measurements for durations; zero monotonic offsets in today's bridge mean
unavailable, not that every event happened at the run's start.

`worker_id` is a run-local scheduling lane. It is neither a physical host nor
a thread identifier. Host samples describe the reporting host, not each worker,
and must not be charged to an individual action. Distributed samples need an
explicit resource identity, measurement interval, and host/container/process
scope before they can share a series. Missing measurements must not become
zero usage.

New measurements should use explicit field presence so a measured zero is
distinct from unknown. Existing implicitly present scalar fields must retain
their documented fallback meaning. Cache probe and transfer timing should be
separate from process execution; `execute_ms` currently includes more than
execution and cannot support claims about compiler time by itself.

## Remaining contract work

1. **Protect lifecycle events before the reporter.** The reporter retains
   creation and finalization after subscription, but the producer broadcast
   can still lose them under sustained lag. The separate cumulative loss
   counter makes this visible, but the producer needs a reliable lifecycle
   lane before a complete-trace claim is justified.
2. **Server liveness and recovery.** A server must distinguish an
   active quiet run, lost reporter, incomplete final drain, and completed build.
   An in-memory client cannot promise recovery after process death; durable
   spooling is a separate opt-in policy with disk and retention budgets.
3. **Conformance against the real projector.** Test atomic gap application,
   duplicate prefixes, divergent duplicates, rejected stale events, expired
   runs, authentication on all methods, invalid fields, and unknown event
   variants. Client integration tests cover transport behavior, not server
   transaction durability. Retention expiry needs an unambiguous response that
   cannot recreate an expired run under the same identity.
4. **Complete correlations.** Cache transfers lack decision identity and
   attempts have no declared action dependencies or separate cache-probe
   timing, so the critical path remains incomplete. Content digests use raw
   bytes with an explicit algorithm; per-action keys survive retained outcome
   reuse. Future retry policies must emit distinct attempts and identify the
   selected outcome.
5. **Live test cases.** Some runners expose only an end-of-run result report.
   Those case outcomes are retrospective and must not be presented as live
   progress. The Apple test runner now translates observed XCTest and Swift
   Testing output into per-case verdicts and durations. A real NetNewsWire
   parser run reported 354 passed cases and no unknown outcomes. Cases absent
   from native output remain unknown. Streaming test runners should emit starts
   and completions at execution time.
6. **Privacy on the server.** Workspace names are disclosed only by explicit
   opt-in and hash-key length and expiry are validated before use. The server
   must validate safe literals against the versioned allowlist and apply its
   own access control to dashboard links and projected records.

These are concrete prerequisites for a durable public contract, not promises
that the new schema has already implemented them.

## Compatibility and rollout

The new dashboard fields and target phase variant use new field numbers.
Existing numbers remain intact. The content digest fix corrects hexadecimal
text previously sent in the raw digest field; coordinate its rollout with the
projector and normalize retained legacy values before comparing them. Deploy
server support for target
phase completion before enabling its projection; an older consumer may omit
those spans but must not reinterpret them as actions. Consumers should preserve
unknown fields in raw retained events and tolerate unknown enum values. A
missing decoded event payload may represent a future variant rather than a
malformed event. Raw retention must not depend on decoding and re-encoding
through a generated client that discards unknown fields.

Follow the [Protocol Buffers evolution guidance](https://protobuf.dev/best-practices/dos-donts/)
and [explicit field presence guidance](https://protobuf.dev/programming-guides/field_presence/):
never reuse removed field numbers, do not change existing field types or
semantics, and reserve deleted names and numbers. Add cross-version fixtures
before claiming compatibility with independently deployed projectors.

The [Bazel build event stream definition](https://github.com/bazelbuild/bazel/blob/master/src/main/java/com/google/devtools/build/lib/buildeventstream/proto/build_event_stream.proto)
already models linked events, test results, actions, and artifacts. Once's
advantage should be demonstrable execution causality, cache explanations,
resource accounting, and recoverable delivery. A larger list of event types
alone does not establish that advantage.

## Validation

Client and producer checks run during this review:

- `mise exec -- cargo test -p once-events-client -p once-cli -p once-frontend`
  passed, including transport replay, attempt identity, privacy configuration,
  retained action keys, and target-kind examples.
- `mise exec -- cargo clippy -p once-events-client -p once-cli -p once-frontend
  --all-targets -- -D warnings` and the formatting check passed.
- `mise exec -- cargo build --release -p once-cli` passed. The focused
  Shellspec command, run, and test suites passed with 63 examples, three
  optional microsandbox skips. The full Shellspec suite stalled in its
  translator and was stopped, so it has no pass result.
- The documentation compiler regenerated the event reference from the wire
  definition, and the local documentation pages rendered in headless Chrome.
- A ripgrep workspace build and its 290-case test target passed. A NetNewsWire
  parser build and test process passed with 354 observed cases reported as
  passed and no unknown cases. A full NetNewsWire application
  build lacked a generated project secret. Mise workspace discovery passed,
  while its build was blocked by a newer required Mise version.

The production dashboard and server projector were not exercised. No claim of
production server compatibility or full workspace validation follows from these
client checks.
