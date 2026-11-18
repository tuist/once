# Executor Hardening: Implementation And Remaining Gates

This records implementation work following the
[failure analysis](0014-executor-failure-modes.md) and
[executor model](0014-executors-and-resource-domains.md). It distinguishes
safeguards for existing execution paths from acceptance criteria for features
Struo does not yet implement. It does not authorize broader cache equivalence,
remote graph placement, speculative execution, or an exactly-struo claim.

## Design Iteration

Claude challenged the design and implemented scoped cache, admission, local
isolation, graph recovery, and hosted-lifecycle changes. The integration review
also identified native descendants surviving signals, uncacheable graph paths
bypassing network policy, a server-selected Daytona origin receiving credentials,
and blocking publication outliving its staging owner. These are correctness and
ownership defects, not reasons to weaken requirements or treat failures as hits.

Further reviews covered subscriber cancellation during shared initialization and
producer repair, cycles in recovery, process-group registration races, and
resource/lock ordering. They also found quiet E2B setup incorrectly treated as a
transfer stall, an undersized cache carve-out, and upload EOF assumptions that
prevented the acknowledgement clock from starting. Linux descriptor inheritance
and receipt bypasses were closed. Portable filesystem operations, local sandbox staging/removal, output capture
and restore-time validation now retain admission in their physical blocking
owners. Validation fan-out is bounded by the admitted buffer allowance.

Validation results must be recorded after those edits settle, not inferred from
individual components passing earlier tests.

## Coverage Of The Findings

| Failure category | Safeguard in the existing model | Remaining release gate |
| --- | --- | --- |
| Cache identity | Legacy action encoding/domain regression; provider, adapter, environment, account, and project remain partitioned; operational I/O limits stay outside identity | Verified immutable tool/environment identities, stable execution roots, explicit trust namespaces before widening reuse |
| Ineligible fallback | One selected executor, no implicit fallback/race; unsupported isolation, output, observer, and account/project requirements fail before provider access | Independently eligible candidates and isolated publication for any new fallback/racing policy |
| Client resource growth | Hosted quota plus host client reservation; bounded control bodies/frames/metadata, streamed capture, shared staging admission, archive byte/entry caps; cooperative CPU budgets for test children | Machine/account-wide quotas, remote graph fan-out, VM overhead and hard process enforcement |
| Cancellation ownership | Blocking file/archive/install/capture work owns staging and action leases; registry-owned initialization and repairs survive first-subscriber cancellation; independent hosted creation and cleanup; process-group termination | Cancellation stress for future shared workers and higher fan-out; persistent workers are not implemented |
| Transfer deadlock | All-or-nothing staging acquisition; input phase released before output phases; growing capture admission fails rather than waits while holding partial capacity; bounded publication lock waits | Stress and liveness tests for any future shared transfer scheduler and event/action upload composition |
| Absence versus outage | Classified cache failures; only confirmed missing blobs trigger bounded producer recovery; corruption is verified and quarantined; idempotent retries and timed initialization cooldown | Negotiated native REAPI recovery and explicit remote materialization policy |
| Uncertain execution | No blind create or command retries; durable intent, operation marker, authenticated reconciliation, retained uncertain records and finite hosted lifetimes | Duplicate-safe REAPI actions, observation recovery and optional cancellation handling |
| Persistent reuse | Durable cache metadata; output restoration separated from materialized flags; build receipts distinguish deferred intermediates; per-invocation producer provenance and shared repair outcomes; fingerprint-only restoration/reuse refusal; local target memo keys bind dependency contents | Persisted remote producer/location provenance |
| Estimates versus guarantees | Required minimum admission fails instead of clamping; native network denial fails closed; concurrency/memory estimates explicitly not hard limits | Required cgroup/container enforcement, unavailable-controller tests, generic target-kind requirement declarations |
| Interoperability | Hosted control responses bounded/sanitized; redirects disabled; Daytona toolbox restricted to configured origins; ownership listings verify markers | Native REAPI negotiation, independent implementations, layer-7 proxy/GOAWAY/rotation tests |
| Maintenance and diagnostics | Bounded reconciliation passes, live-owner locks, quarantine; structured admission and hosted phase logs without command output, raw error bodies, or environment values | Recovery diagnostics and maintenance fairness stress tests; enforce any stronger machine-wide guarantee before claiming it |

## Existing Execution Safeguards

### Hosted I/O And Publication

`RemoteIoLimits` controls response/frame bytes, archive bytes, archive entries,
metadata, staged bytes, transfer stalls, and optional command-idle detection.
Limits are normalized against the admitted client memory estimate. Defaults
currently cap each archive at 4 GiB and shared transfer staging at 8 GiB.
Captured command output has a separate 1 GiB per-action cap and an 8 GiB shared
capture pool. Capture allowance is reserved before E2B creation, so ordinary
contention waits before paid work rather than failing midway. Daytona bounds the
combined stdout/stderr download against the same per-action output cap. Separate
capture and transfer pools prevent acquisition cycles; their disk allowances are
additive, not one claimed 8 GiB total. Limits are runtime policy, not new manifests
or guest enforcement.

E2B parses incremental Connect/JSON-line events for setup and execution and sends
stdout/stderr through bounded pipes to CAS. Setup bounds total traffic, not only
individual frames. Quiet gaps between frames are allowed; incomplete frames
remain subject to stall detection. Capture capacity is acquired without awaiting while holding
partial capacity. Daytona redirects streams into guest files and downloads them
to leased temporary staging before streaming into CAS. Large logs no longer
require a same-sized command-response JSON allocation.

Input planning accounts for path metadata, entries, tar overhead and file sizes.
The writer enforces the plan even if an input grows afterward. Output preflight
bounds extended tar headers before normal decoding; extraction validates paths,
entry count and expanded bytes. Blocking work owns temporary directories and
leases, including after the observing future is cancelled. Publication shares a
bounded workspace lock with cached restoration and rolls back partial installs.

An action timeout includes reconciliation and the hosted work sequence, not a
fresh timeout for every phase. Cleanup has its own bounds outside that deadline
and remains owned after an action timeout. Body inactivity defaults to 30 seconds;
after the declared upload payload has been produced, acknowledgement gets a
separate five-minute bound. Known-length uploads do not depend on HTTP clients
polling EOF to start that clock. A transfer stall is distinct from an otherwise
quiet command.

Request JSON uses a bounded writer that counts encoded escaping costs before
sending. Structured phase logs record elapsed time and classified outcomes, not
secret-bearing URLs, response bodies, environment references or command arguments.

### Portable Artifact Fetches

Checksum-pinned archive fetches use the same runtime I/O policy. The absolute
30-minute download deadline covers headers, staging admission, temporary archive
creation, body progress and private writes. The 30-second stall bound remains
separate. Checksum verification and extraction follow download completion, rather
than timing out an uncancellable task that is already modifying output staging.

ZIP metadata preflight validates the final EOCD/ZIP64 records before constructing
the decoder, caps central-directory bytes and entries, and budgets decoder reads.
Fallback record scanning prevents older signatures from bypassing the entry cap.
Multi-disk archives, inconsistent trailing records and prefixed data are refused;
an embedded ZIP record declaring an excessive count can also trigger refusal.
The per-record allowance is a conservative estimate, not a hard RSS guarantee.
Expanded bytes and symlink metadata have independent caps. Staging reserves the
archive plus its expansion bound in one acquisition before writes. Known-length
archives cap expansion at the smaller of 4 GiB and 64 times the declared length,
so small downloads can run concurrently. Unknown-length archives retain worst-case
reservation and can serialize. Highly compressible archives exceeding the ratio
are refused. The ratio and deadline are runtime policy outside action identity;
exact expansion planning and throughput-aware policy remain future improvements.

### Hosted Ownership

Each managed create persists an intent under `.struo/executors`, attaches a unique
metadata/label marker, then persists returned sandbox IDs before handing ownership
back. The create task survives subscriber cancellation and cleans up late responses.
Deletion retains admission through bounded attempts and keeps failed/uncertain
records for a later authenticated, endpoint-scoped reconciliation pass.

Reconciliation never deletes unlabelled or foreign sandboxes, skips live owners,
and accepts absence only after a complete filtered listing and settlement delay.
A create is not retried just because its response was lost. No HTTP status proves
that creation had no side effects, including 400, 403, 404, 409 and 422; only a
local pre-send metadata-size refusal clears the intent as definitely rejected.
Credentials are not stored in the journal. Restart deletion rechecks the current
exact marker even for persisted IDs. An empty listing with changed credentials
never establishes absence, even after settlement.

Admission caps the journal at 256 records and 576 directory entries before paid
creation. Revoked or replaced credentials can leave records unresolved indefinitely
and eventually refuse new work. This is a deliberate safety/availability tradeoff.
A lost journal, an unkillable guest, runtime shutdown, or refused deletion can still
leave an orphan; none of this establishes a hard physical fleet bound.

### Native Isolation And Admission

Native commands run in supervised process groups. Timeout/cancellation stops the
group; inherited output pipes have bounded drain time. On Unix, leader exit is observed without reaping before group ownership is
released. Signal-safe registration handles spawning and unregister/reap races.
A first streaming signal is recorded as cancellation before action futures are
dropped; final exit kills remaining registered groups. Non-streaming invocations
also stop their groups. Windows currently terminates only the leader, so these
Unix descendant guarantees must not be advertised as cross-platform guarantees.

Linux network denial validates syscall ABI and rejects alternate-ABI, io_uring,
and descriptor-receipt bypasses. It probes close-on-exec range support before
spawn, marks non-stdio descriptors close-on-exec without disrupting the exec-error
pipe, and rejects non-Unix sockets on standard streams. Linux 5.11 and an allowing
container profile are required. Plain Unix reads/writes remain supported; receive
messaging and descriptor-passing IPC do not. A permitted external Unix peer can
still proxy work, so socket isolation is not proof of no delegated network activity. macOS denies Internet sockets including loopback while
preserving local Unix-domain IPC. Unsupported enforcement fails closed. Sandboxes
have unique execution directories rather than sharing only an action digest.

Host admission preserves fairness between conflicting waiters, removes cancelled
claims, and separates required minimums from normalizable estimates. Named slots
remain shared. Test-batch children divide the parent's CPU capacity through
`--cpu-limit`; arbitrary nested tool threads still require cooperative declarations.

### Cache And Graph Recovery

Cache APIs preserve absence, refusal, unavailability, corruption and protocol
failures as distinct categories. Read integrity checks stream content hashes;
corrupt local content is quarantined rather than restored. Only idempotent cache
RPCs retry under a bounded total budget. Transient initialization failures have a
timed recovery probe instead of poisoning a process permanently.

Cloned remote cache providers share memory and scratch-byte admission. The CLI
carves cache memory out of its command budget, rather than granting cache work
an additional full allowance alongside action reservations. Required buffers
must not be silently clamped or untrusted declared sizes allowed to grow disk
without admission. Capacities round down and requests round up to 4 KiB units.
The minimum CLI cache allowance remains 32 MiB + 64 KiB. Streamed ingestion
includes bounded codec and producer queues in that allowance, reducing its
message ceiling when necessary rather than adding buffers outside the budget. Finite budgets reserve one eighth, clamped to
that floor and 256 MiB, and give actions exactly the remainder. Totals at or below
the floor fail rather than creating a zero action budget that means unbounded.
MCP/runtime sessions reserve headroom for later provider changes even if initially
local. A fault-server test downloads a 33 MiB blob at exactly the floor and proves
that one byte less fails before opening the download stream.

Core cache reads remain strict: exhausted outage/refusal failures propagate, not
ordinary misses and not producer repair. Choosing local-only caching is explicit
policy. The SDK documentation records these behavior changes and the additive
public error classification surface; exhaustive error matches need review.

Action hashing streams canonical JSON through a 16 KiB buffer into BLAKE3 while
preserving the legacy domain and bytes. Supported provider image/template overrides
are bound into execution identity before cache lookup, including named adapters
and annotated scripts. Raw unbound overrides fail closed. Mutable references still
do not establish immutable environment equivalence.

Graph recovery records which action produced a deferred path/digest, restores
inputs before execution admission, and reruns only on confirmed `BlobNotFound`.
Repairs have per-producer, depth and invocation budgets and reject changed output
bytes. Registry-owned tasks spend the budget once, survive caller cancellation,
share results with all waiters, and reject self/cyclic waits. Panic, abort or optional
repair-deadline expiry fails without silently repeating uncertain work. Stale materialization claims are cleared, and build receipts do not demand
that every deferred intermediate remain on disk. This is local graph repair, not
remote graph provenance or remote placement.

## Graph Caller Ownership Follow-up

A later caller-path review found that graph actions and producer repairs held a
host permit only in their observing future, while entering uncached core work
without an owned execution lease. Cached output restores lacked the same bridge,
and legacy capability fallback constructed a default runner instead of using the
invocation's limits. Earlier green checks did not cover those integration edges.

Graph misses, uncached portable work, command preparation, output capture and
producer repairs now forward owned execution leases. Registry clones share one
executor resource context bound to the exact host pool; configuration happens
before work starts. Every restore attempt acquires materialization admission and
releases it before any producer repair. Legacy capabilities use the same resource
context, including CPU and memory limits. Argument-file preparation is admitted
as materialization work before action execution.

Cross-process action locks use independently opened file handles retained by the
lease before cloning. Cancellation and optional repair deadlines cannot release
the action lock or host reservation under a running blocking writer. The core
caller-admitted entry point refuses mismatched permits and non-native graph
placement; this does not introduce remote graph execution. Leases and retained
owners are runtime state, outside action serialization and cache identity.

Action admission ends when core execution finishes. The observing cache-publication
phase retains the action lock, while cache requests are bounded by the carved
cache-transfer allowance. An archive waiting for staging can retain its host reservation; no
staging holder waits to acquire host admission, so this does not introduce a
hold-and-wait cycle. Unknown-length archive serialization remains deliberate.

FIFO regressions cover cached and uncached graph cancellation and repair deadline
expiry, checking both capacity and lock lifetime until physical work stops. Other
regressions cover shared staging refusal, a 16 MiB ZIP metadata allowance under a
64 MiB action budget, legacy capability admission, executor mismatch, and cache
identity parity. Removing lease forwarding fails four regressions; removing the
retained lock owner fails both lock-lifetime regressions. Both mutations were
restored before final validation. Direct-spawn capture retains the legacy raw-file,
directory-manifest and symlink formats rather than adopting core's restorable
artifact envelopes. Pinned digest tests exercise the production path, including
dangling links; an owned capture-worker cancellation test checks the bridge.

Those legacy uncached manifests contain hashes, not restorable directory contents.
The subsequent adversarial review confirmed that restoring them after cleanup
could install manifest text as a regular file and poison consumer results. The
fingerprint-only restoration and historical-result guards below supersede the
previous decision to leave this as an unsupported compatibility limitation.
Linux/network-filesystem and Windows lock semantics, live hosted interoperability,
lifecycle clock skew, ZIP size misreporting and aggregate filesystem overhead
retain their own validation gates.

### Fingerprint-Only Outputs And Historical Poisoning

A real three-action regression confirmed the failure: an uncacheable unsandboxed
command produced a symlink, another action cleaned it, and a consumer successfully
read its manifest as ordinary file contents. A directory producer similarly left
an incorrect regular file at the directory path. Producer fingerprints and
consumer action keys stayed unchanged, allowing bad results to persist in the
cache.

Direct capture now records insert-only invocation evidence keyed by exact output
path and digest. Capture and verification share the same typed legacy fingerprint
function. Before any core restore or prefetch, a materialization-admitted blocking
worker checks each fingerprint-only output against its current filesystem type
and contents. Verified outputs are omitted from restoration. Missing, changed or
wrong-type outputs refuse with the path, producer identifier and an actionable
hint. The refusal is not missing-cache evidence, never runs a producer again, and
happens before installing other outputs from the same result. Unmaterialized
fingerprint inputs also validate during preparation, before computing an action
key or querying the cache, without nested admission.

Historical action-cache entries cannot be trusted merely because the current
output is intact: a formerly cleaned graph and an intact graph can have identical
consumer keys. Consumers of fingerprint-only declared or implicit same-target
inputs therefore skip both the initial cache probe and the contended-lock
re-probe, while retaining ordinary admitted execution and publication. This is an
eligibility decision, not authorization to run after a cache outage or refusal.
Local and remote publication retain their existing failure policies. These
consumers intentionally recompute on each invocation; ordinary raw-file and core
artifact inputs keep their reuse behavior.

Whole-target reuse also excludes fingerprint-only dependency inputs. An
intermediate target can replace the fingerprint path with a restorable artifact,
hiding that original evidence from later targets. Local target-outcome memo keys
now bind canonical dependency path/content digests in addition to analysis and
producer action digests, through a versioned, incremental BLAKE3 encoding with
length-prefixed fields and counted sections. Corrected intermediate contents
invalidate historical downstream records even when producer action keys remain
unchanged. Only local optimization keys change; action encoding and digest
domains, artifact bytes, persisted record shape and the cache-centered SDK remain
unchanged. Older local memo keys miss safely.

Evidence remains insert-only even if a later raw-file output has the same path
and bytes as an earlier directory or symlink fingerprint. Clearing it would let
older input maps restore the manifest as a file again. Such exact-key ambiguity
conservatively refuses restoration for the invocation. Raw files merely starting
with manifest prefixes, at unregistered keys, remain ordinary restorable files.
Legacy fingerprints still exclude file modes and special directory entries, and
raw legacy file restoration retains its existing mode policy.

Regressions cover cleaned symlinks/directories, unchanged live outputs, wrong
filesystem types, absent descriptor blobs, refusal classification and no repair,
no partial installation, exact-key ambiguity, literal-prefix files, exact nested
children, cross-target inputs, historical poisoned hits with intact explicit and
implicit inputs, and whole-target/intermediate-hop replay. Removing the restore
split fails four tests; enabling consumer reuse fails the seeded-poison test;
removing the target exclusion fails its historical-record test; omitting content
digests from memo keys fails two key/intermediate-hop tests. All mutations were
restored before final acceptance.

## Validation Status

The integrated hosted fault suite passed 66 tests after managed creation was
wired into both adapters. It covers oversized/chunked bodies, stalls, protocol
frames, large streamed logs, input growth, 50,000-entry input planning, archive
limits, rollback, blocking-install cancellation, late create responses, lost
create recovery, foreign markers, live-owner locks and failed-deletion repair.

Before the graph caller-ownership follow-up, the combined tree passed:

- `mise exec -- cargo test --workspace`, including 423 core unit tests, 522 CLI
  unit tests, cache fault/persistence tests and the cancellation/graph integration
  suites. Ignored child-process helpers are exercised by their parent tests;
  live-endpoint and unrelated opt-in tests retain their own gates.
- `mise exec -- cargo test -p struo-core --all-targets --no-default-features`.
- Workspace and no-default-feature core Clippy with `--all-targets -- -D warnings`.
- `mise exec -- cargo fmt --all -- --check` and `git diff --check`.
- `mise exec -- cargo build --release`.
- `STRUO_RUN_MICROSANDBOX_SPECS=1 mise exec -- shellspec`: 328 examples, zero
  failures, seven unrelated Cargo/container opt-in skips. All three real
  Microsandbox execution/input-output/cache-restoration examples ran.

Claude's initial integrated adversarial review found no production blockers, but
the later caller-path findings superseded that blanket conclusion. The follow-up
review found the corrected ownership, lock and restore ordering sound by code
reading. After the compatibility correction and pinned production-path tests,
Claude repeated the graph admission/capture/capability regressions three times and
reported no remaining blockers in those ownership fixes. A subsequent fresh
review found the fingerprint-only restoration and historical-replay bugs above,
so that earlier conclusion is not acceptance of the later corrected tree. The
platform and live-environment gates remain explicit.

The caller-ownership corrected tree, before the later fingerprint-only fixes, passed:

- `mise exec -- cargo test --workspace`: 425 core unit tests and 532 CLI unit
  tests passed, together with workspace integration and documentation tests.
- `mise exec -- cargo test -p struo-core --all-targets --no-default-features`.
- Workspace and no-default-feature core Clippy with `--all-targets -- -D warnings`.
- `mise exec -- cargo fmt --all -- --check` and `git diff --check`.
- `mise exec -- cargo build --release`.
- `STRUO_RUN_MICROSANDBOX_SPECS=1 mise exec -- shellspec`: 328 examples, zero
  failures, seven unrelated Cargo/container opt-in skips. Real Microsandbox
  execution, input/output transfer and cache restoration ran.

An earlier overlapping ShellSpec run used a pre-correction binary and is not used
as acceptance of that caller-ownership tree. Its acceptance used the rebuilt
corrected binary and complete suite above, not a partial run or mutation tree.
The later fingerprint-only fixes have their own validation below.

Deterministic framing/deadline tests passed 20 full no-default-feature core rounds.
Later download, admission and cancellation regressions were repeated, and mutation
checks rejected the old ownership, acknowledgement and expansion policies.

Earlier checkpoints, scratch exports and the initial ten-minute ShellSpec cutoff
are not used as final acceptance. Linux code was cross-compiled and linted, but
live seccomp and real hosted/proxy interoperability require their own environments
and are not established by macOS/mock-server tests. ZIP size-misreporting mutation
coverage and aggregate accounting of symlink/directory filesystem overhead remain
focused follow-up gates; regular file expansion, metadata and entry counts have
separate enforced bounds.

### Fingerprint-Only Follow-Up Acceptance

Claude reviewed the restoration guard, rejected unsafe evidence clearing, checked
historical action/target replay and identified the hidden intermediate-hop case.
The corrected final review found no remaining blockers in these fixes, including
the incremental, length-prefixed local memo key. That conclusion covers this
change, not the platform, hosted-provider or future-feature gates above.

The final settled tree passed:

- `mise exec -- cargo test --workspace`: 549 CLI unit tests and 425 core unit
  tests passed, together with workspace integration and documentation tests.
- `mise exec -- cargo test -p struo-core --all-targets --no-default-features`.
- `mise exec -- cargo clippy --workspace --all-targets -- -D warnings`.
- `mise exec -- cargo clippy -p struo-core --all-targets --no-default-features -- -D warnings`.
- `mise exec -- cargo fmt --all -- --check` and `git diff --check`.
- `mise exec -- cargo build --release`, rebuilt after the final production edits.
- `STRUO_RUN_MICROSANDBOX_SPECS=1 mise exec -- shellspec`: 328 examples, zero
  failures, seven existing Cargo/container skips, 823.11 seconds. Live
  Microsandbox execution, declared input/output transfer and cache restoration ran.

The 17 new regressions and four targeted mutations distinguish actual protection
from a green baseline. Preliminary checks run before the intermediate-hop fix,
and checks overlapping mutation edits, are not final acceptance. All mutations
were restored before the settled-tree commands above; ShellSpec used its rebuilt
release binary.

### Streaming Integration Acceptance

Integrating the newer streaming and selective-materialization paths retains
executor leases and their lifetime owners in scoped filesystem workers,
including opportunistic extra workers. Producer recovery still runs only for
confirmed missing blobs, outside consumer admission. It now recreates deferred
argument files under the producer lease, so a cached producer need not have
written those files before a later repair. A regression fails without this
recreation and passes with it.

Read-only staging verifies both pinned raw blobs and decoded blobs before
returning a reader, with one remote refetch after local corruption. Download
workers retain transfer reservations after observer cancellation. Raw and
encoded scratch files are admitted together, and encoded body growth is
bounded. This deliberately raises the staging requirement to slightly more than
twice the declared raw size; it does not change action identities, artifact
bytes, or the cache-centered SDK scope. The SDK guides describe the accounting
and compatibility cost.

The integrated settled tree passed:

- `mise exec -- cargo test --workspace`: 579 CLI, 471 core, and 262 CAS unit
  tests passed, together with workspace integration and documentation tests.
- `mise exec -- cargo test -p struo-core --all-targets --no-default-features`.
- `mise exec -- cargo clippy --workspace --all-targets -- -D warnings`.
- `mise exec -- cargo clippy -p struo-core --all-targets --no-default-features -- -D warnings`.
- `mise exec -- cargo fmt --all -- --check` and `git diff --check`.
- `mise exec -- cargo build --release`, rebuilt after the production changes.
- `STRUO_RUN_MICROSANDBOX_SPECS=1 mise exec -- shellspec`: 328 examples, zero
  failures, seven existing Cargo/container skips, 614.30 seconds.

The earlier adversarial review applies to its reviewed tree. These integration
checks do not supersede the separate live-platform and future-feature gates.
