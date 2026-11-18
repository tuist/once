# Executor Failure Modes: Evidence And Safeguards

This is the failure-analysis companion to
[RFC 0014](0014-executors-and-resource-domains.md). It examines public Bazel,
Buck, and Buck2 issue discussions, fixes, and the Remote Execution API contract,
not just their intended architecture. Sources were reviewed on 2026-10-11.

An open issue is evidence of a reported failure, not proof that every current
release has the defect. A closed issue is not necessarily a fix. Where available,
maintainer explanations, reproducer confirmations, and fixing commits take
precedence over the initial report. Buck findings below describe historical
Buck behavior, not Buck2 behavior.

## Implementation Status

The **Struo now** assessments below record the original resource-domain boundary
reviewed for this research. The subsequent
[hardening implementation](0014-executor-hardening.md) tracks the safeguards,
fault tests, and remaining feature gates. Historical evidence is retained here
rather than rewritten as proof that every proposed gate has passed.

## Conclusions

The resource-domain boundary is necessary but insufficient. Most expensive
failures happen where individually reasonable subsystems compose: graph fan-out
with client admission, cancellation with shared work, cache metadata with blob
retention, or transport retries with execution ownership.

Struo should preserve these invariants:

1. Placement never establishes result equivalence by itself.
2. Every execution branch independently satisfies the action's requirements.
3. Every phase has a resource budget, including work that never executes a command.
4. Shared work has an owner independent of any one subscriber.
5. Cancellation of observation is not proof of cancellation of execution.
6. Cache metadata, blob availability, and local materialization are distinct states.
7. A transport outage is not a cache miss or evidence of missing inputs.
8. Reusable results survive process restarts without depending on scheduler state.
9. Required guarantees fail closed; estimates are explicitly described as estimates.
10. Protocol interoperability and operational diagnostics are acceptance criteria.

## 1. Cache Identity Can Be Unsound Or Needlessly Fragmented

**Evidence.** [Bazel #4558](https://github.com/bazelbuild/bazel/issues/4558)
describes identical action hashes when undeclared system compilers or libraries
differ. The discussion includes toolchain hashing and cache namespace workarounds.
This is a correctness risk, not merely a performance problem.

[Bazel #26357](https://github.com/bazelbuild/bazel/issues/26357) reports the
opposite: host-derived configuration, output paths, and shell environment prevent
sharing even when execution and target platforms are identical. The reporter
ultimately describes configuration workarounds; the thread does not prove one
unresolved hashing bug explains every miss.

[Buck #2405](https://github.com/facebook/buck/issues/2405) is a useful counterpoint:
a maintainer explains that absolute working directories are keyed intentionally
because tools can embed them in outputs. Removing paths from a hash is not safe
unless execution paths or tool behavior are normalized too.

**Struo now.** Provider bindings and resource estimates remain conservatively
keyed. Native commands clear the inherited environment and install their declared
environment, but this does not identify every system tool, library, or file a
command can read. A mutable hosted image or template name does not become
immutable because it is in the digest. This change deliberately does not claim
cross-executor equivalence.

**Required safeguard.** Resolve immutable tool and environment identities, a
stable execution root, and cache trust namespaces before widening sharing. Keep
client-host, execution-platform, and target-platform identities separate. Normalize
unordered protocol fields, not semantically ordered arguments. Any execution
parameter omitted from identity needs an explicit claim that changing it cannot
change the output, backed by tests; parallelism is not automatically harmless.

**Regression.** Change a tool's bytes without changing its path or version label
and require a miss. Change only the client OS, checkout location, or provider alias
for an explicitly equivalent remote action and require identical prepared action
identity. Test a tool that embeds its working directory before permitting path
normalization. These are follow-on gates, not claims about current digest behavior.

## 2. A Valid Remote Platform Does Not Make The Host Eligible

**Evidence.** [Bazel #25221](https://github.com/bazelbuild/bazel/issues/25221)
reports dynamic execution racing a GPU-constrained action on a CPU-only host,
despite the configured remote execution platform. The discussion includes strategy
and toolchain workarounds, rather than evidence that all platform checks can be
ignored safely.

[Buck2 #358](https://github.com/facebook/buck2/issues/358) discusses undeclared
headers and ambient environment in local builds, illustrating how local and
remote behavior can diverge. It also contains a maintainer warning that routing
local sandboxing through a localhost RE service would conflict with assumptions
about a single CAS and add input-transfer overhead. A uniform interface need not
mean every local operation should traverse gRPC.

**Struo now.** Graph execution remains local. Scripts select one executor, unknown
adapters fail, and unsupported adapter requirements are rejected. There is no
implicit host fallback or speculative race.

**Required safeguard.** Check each candidate's tools, execution constraints,
isolation, and output semantics before admission. Fallback may change placement,
not weaken requirements. A command failure must not be silently reclassified as
an infrastructure failure to obtain a successful answer elsewhere. Local execution
can reuse prepared action semantics without depending on a remote transport.

**Regression.** A remote-only capability never starts a local branch, including
on authentication failure, queue timeout, or service outage. Deliberately divergent
local and remote command results must surface the divergence, not pick success.

## 3. Remote Concurrency And Client Resource Safety Must Both Hold

**Evidence.** [Bazel #6394](https://github.com/bazelbuild/bazel/issues/6394)
reports fully cached builds throttled by CPU-oriented parallelism. Its discussion
also records increased client memory consumption and a rollback during attempts
to make remote work asynchronous. Removing the CPU bottleneck is not permission
to create unlimited requests.

[Bazel #10443](https://github.com/bazelbuild/bazel/issues/10443) documents nested
tool parallelism: admitting N commands can start N times M child processes.
[Buck2 #916](https://github.com/facebook/buck2/issues/916) reports repeated remote
file downloads, CDN rate limiting, and file-descriptor exhaustion on large
unvendored dependency sets, even with deferred materialization and remote consumers.

**Struo now.** Hosted requests reserve client memory rather than guest CPU/memory,
and cloned runners share a hosted quota. Cache output restoration has memory-only
admission. However, the default hosted quota is still derived from host parallelism,
graph batching and event worker slots still assume local work, and independently
constructed runners or processes do not share budgets. Hosted command responses
are buffered in full. Reservations are not hard caps on their size.

**Required safeguard.** Budget queued metadata, cache RPCs, transfers, open files,
staging disk, capture, and materialization independently of guest execution. Bound
both request count and in-flight bytes. Hash-pinned downloads should reuse verified
content where available; freshness checking should be an explicit policy, not
mandatory work on every restart. Nested tool concurrency needs declared aggregate
estimates or a cooperative budget interface. Keep ecosystem-specific concurrency
flags in target kinds, not generic Rust executor branches.

The host pool currently admits whichever request fits, so small client requests
can starve a large local request. Remote-before-host acquisition prevents queued
remote work from pinning host memory, but requests waiting for host memory or a
named slot can occupy remote quota. This can reduce utilization without violating
capacity. Fairness and phase ownership need an explicit scheduling policy.

**Regression.** Run a fully cached graph and a mixed local/remote graph with one
host CPU slot; remote progress should follow its configured quota, while memory,
file descriptors, and staged bytes remain bounded. Flood the pool with small
requests and require an older large request to progress. Include nested tools
and slow or oversized output producers.

## 4. Cancellation Can Destroy Shared Progress Or Leak Slots

**Evidence.** [Buck2 PR #1121](https://github.com/facebook/buck2/pull/1121)
removed lower-layer `FindMissingBlobs` deduplication after cancellation of the
originating task caused other waiters to fail with a closed channel. The PR was
merged; the replacement deduplication lived at a higher layer.

[Bazel #26292](https://github.com/bazelbuild/bazel/issues/26292) describes a
response/cancellation race exhausting multiplex worker slots. The reporter
confirmed a fix with a reproducer, and maintainers recorded its inclusion in the
8.4.0 line. It was a lifecycle interaction between dynamic execution, workers,
and cancellation, not simply a semaphore configured too small.

**Struo now.** Sandbox cleanup holds a shared execution permit after foreground
cancellation. This protects admission through the cleanup attempt, but does not
implement subscriber-independent ownership for future shared uploads, execution
operations, or worker requests.

**Required safeguard.** Shared operations own their task, completion state, and
lease independently of the first waiter. Cancelling a subscriber removes that
subscription, not another subscriber's ability to finish. Completion and
cancellation must converge on one terminal transition even if a response arrives
before a cleanup waiter is registered. Cancelled tasks must also be joined or
supervised; spawning a task is not lifecycle management.

**Regression.** Cancel the first of several deduplicated callers while the others
wait. Deliver completion before, during, and after cancellation registration.
Repeatedly cancel work and prove all slots return and task counts remain bounded.

## 5. Transfer Limits Can Deadlock The Work That Releases Them

**Evidence.** [Bazel #19473](https://github.com/bazelbuild/bazel/issues/19473)
was fixed by [commit f2fe4a2](https://github.com/bazelbuild/bazel/commit/f2fe4a29914b19bee6e1d51129fbdb5e03afc00a).
The fixing commit explains that a blocking semaphore inside a shared gRPC uploader
let event-artifact limits throttle action-cache uploads and cause thread starvation.
It moved the event limit to nonblocking flow control at the event uploader layer.

[Buck2 #1123](https://github.com/facebook/buck2/issues/1123) explains a different
upload stall: an upstream HTTP/2 bug assigned connection window capacity to streams
that the server had not admitted. High CAS concurrency triggered it. The issue was
closed after an h2 fix. A bounded application semaphore alone did not establish
transport liveness.

**Struo now.** Domain admission uses asynchronous waits and a documented acquisition
order. There is no native REAPI transfer pipeline yet. The hosted action quota is
not a complete transfer or file-descriptor budget.

**Required safeguard.** Give action transfer, event upload, and cleanup explicit
budget ownership. Apply limits before opening files or buffering bodies. Never
block a completion executor while acquiring a permit released by that executor.
Keep a global byte/file-descriptor ceiling even when traffic classes have separate
quotas. Do not hold transport capacity across unrelated nested requests needed
for completion. Review transport-library fixes and negotiated stream behavior,
not just application-level concurrency.

**Regression.** Use a server with a tiny stream limit and small connection windows,
then upload more blobs than either limit. Add slow reads, cancellation, event traffic,
and connection rotation. Require continued progress and bounded open files, not
merely a timeout that eventually aborts the build.

## 6. Missing Blobs And Unreachable Blobs Need Different Recovery

**Evidence.** [Bazel #27929](https://github.com/bazelbuild/bazel/issues/27929)
reports lost inputs under cache, disk-cache, remote, and dynamic execution. The
reporter later could not reproduce consistently and noted possible disk-cache
eviction; the exact cause is not established by the thread.

[Bazel #30259](https://github.com/bazelbuild/bazel/issues/30259) is more specific:
a circuit breaker that stayed open turned transient CAS read failures into lost
inputs; rewinding could not recover while subsequent reads remained rejected.
A maintainer records a fix in 8.8.0 RC1. Recovery components must work together,
not just independently have retries.

**Struo now.** The runner treats a cached result with missing output or captured
stream blobs as incomplete and re-executes. Output restoration stages blobs before
publishing them. A missing output blob is distinguished from other I/O or corruption
errors. This is not yet remote graph provenance recovery for deferred artifacts.

**Required safeguard.** Track result metadata, remote blob location and availability,
and local materialization separately. Preserve producer provenance to repair a
missing deferred input. Retry transient transport failures without declaring a
miss; verified absence can trigger bounded re-upload or producer re-execution.
Corrupt content must not be accepted or hidden as an ordinary miss. Circuit breakers
need timed, single-probe half-open recovery and must preserve the failure category.
Recovery needs a total attempt/deadline budget so eviction cannot loop forever.

**Regression.** Evict a blob after an action-cache hit and before a remote consumer
starts. Separately inject `UNAVAILABLE`, a deadline, corruption, and confirmed
absence. Only absence takes the missing-input repair path. Recover a tripped breaker
without restarting the build, and terminate predictably under repeated eviction.

## 7. RPC Completion Does Not Prove Exactly-Struo Execution

**Evidence.** The [REAPI execution contract](https://github.com/bazelbuild/remote-apis/blob/main/build/bazel/remote/execution/v2/remote_execution.proto)
explicitly permits an action to execute multiple times, potentially concurrently,
with redundant execution continuing after an operation completes. Full Operations
API cancellation support is optional. `WaitExecution` permits resuming observation
of a named operation; dropping the stream is not cancellation acknowledgement.

[Bazel #7328](https://github.com/bazelbuild/bazel/issues/7328) discusses local
winners cancelling remote branches before those branches populate the remote
cache. Continuing abandoned work can improve future hits, but also spends resources.
That is a service/workload policy, not universally a client correctness fix.

**Struo now.** There is no automatic resubmission or racing. Cleanup retains leases
through bounded attempts, but a lost create response, failed deletion, shutdown,
or unkillable VM can leave work behind. Releasing a lease after a failed cleanup
attempt does not establish a physical fleet bound.

**Required safeguard.** Native REAPI execution must be restricted to actions safe
under duplicate execution; externally visible deployment or mutation is a different
contract. Persist operation or sandbox identity early, resume observation when
possible, and reconcile uncertain creates through idempotency keys or provider
lookup where supported. Track cancellation requested, stop confirmed, and stop
unknown separately. Provider lifetimes and reconciliation need to bound orphan
cost. Racing requires separate staging and one output publication claim, even
when losing work is intentionally allowed to populate CAS.

**Regression.** Lose the create response and execution stream, delay cancellation
acknowledgement, fail deletion, and kill the client. Show whether work can still be
running and how it is reconciled. A late losing result must never overwrite winning
workspace outputs or be reported as the selected result.

## 8. Incrementality, Persistent Caching, And Materialization Are Different

**Evidence.** In [Buck2 #547](https://github.com/facebook/buck2/issues/547),
maintainers explain historical rebuilds after daemon termination and distinguish
DICE incrementality from a cross-daemon action cache.
[Buck2 #976](https://github.com/facebook/buck2/issues/976) revisits persistence of
local cache/dep-file state, including abrupt termination. These threads motivate
restart tests; they are not an assessment of every current Buck2 cache mode.

[Buck #342](https://github.com/facebook/buck/issues/342) describes removed outputs
not being recreated while rule metadata remained. The maintainer explains the
assumption that output metadata and files are consistent, and why undeclared side
effects do not fit cached build rules.

**Struo now.** CAS/action-cache reuse is independent of a long-running daemon, and
ordinary runner hits restore outputs. Reusable graph target outcomes also retain
per-action identities. Their persistent validity and deferred output availability
must continue to be checked separately from in-memory graph reuse. Local target
memo keys now bind dependency output contents as well as producer action digests,
so changed intermediate contents invalidate downstream reuse even when the
producer action is unchanged. Legacy direct-spawn directory and symlink outputs
carry invocation-owned fingerprint-only evidence: restoration verifies their
current type and contents or refuses before installing descriptor text. Consumers
skip action-cache reuse, including historical poisoned results with intact inputs,
and direct dependency fingerprints exclude whole-target reuse.

**Required safeguard.** A scheduler restart must not erase valid reusable results.
Persist metadata atomically and revalidate its referenced content. Missing local
restorable outputs need restoration; fingerprint-only outputs need an explicit
refusal, not a manifest installed as file contents. Only confirmed missing cache
content authorizes producer repair. Side effects outside
declared outputs must not be promised on a cache hit. Cache tenancy and digest
function are part of an artifact location, not assumptions hidden in one global
CAS handle.

**Regression.** Build, terminate the process, delete local outputs, and rebuild
using the same persistent cache without rerunning commands. Repeat after interrupting
metadata publication and after evicting one blob. Test separate trusted cache
namespaces containing the same digest without mixing their result authority.

## 9. Estimates Must Not Masquerade As Enforced Requirements

**Evidence.** [Bazel #26062](https://github.com/bazelbuild/bazel/issues/26062)
reports requested sandbox memory limits silently disappearing when cgroups could
not be created. Maintainer discussion explicitly distinguishes best-effort resource
management from required enforcement. The thread does not establish that admission
estimates themselves can prevent OOM.

**Struo now.** Unsupported sandbox-adapter requirements fail before startup. Host
reservations are estimates, not kernel limits; the pool normalizes requests to its
capacity. Microsandbox sizes its guest from the normalized reservation, but this
does not account for all VM overhead or enforce a whole-host process budget.

**Required safeguard.** Keep estimated consumption separate from minimum capacity
and hard enforcement requirements. An oversized estimate can have a documented
exclusive-admission policy; an unsatisfied minimum or unavailable required limiter
must fail, not be clamped into apparent compliance. Report the requested and applied
policy. Do not advertise host memory admission as an OOM guarantee.

**Regression.** Remove enforcement permissions and require a structured failure
before execution when enforcement was requested. Supply a minimum above domain
capacity and require rejection. Verify that ordinary estimates remain identified
as estimates, including when normalized.

## 10. A Direct Backend Test Is Not An Interoperability Test

**Evidence.** [Buck2 #1136](https://github.com/facebook/buck2/issues/1136)
reports transport failures during service/load-balancer rotation and asks for
request-specific retries. The claimed internal-proxy explanation is the reporter's
hypothesis, not an established cause.
[Buck2 #1294](https://github.com/facebook/buck2/issues/1294) supplies a reproducer
for failures behind nginx/AWS ALB and points to transport-library behavior.
[Buck2 #1477](https://github.com/facebook/buck2/issues/1477) reports empty worker
properties with a newer BuildBarn and identifies the migration from deprecated
`Command.platform` to `Action.platform`; a commenter points to a related PR.

**Struo now.** Hosted adapters do not constitute REAPI interoperability coverage.
The executor model intentionally leaves the service's scheduling protocol outside
host accounting.

**Required safeguard.** Negotiate service capabilities and use canonical protocol
encoding, including the supported platform field and digest function. Exercise at
least two independent REAPI implementations and a layer-7 proxy. Retry idempotent
reads and content-addressed writes with jitter, server retry hints, and total
budgets. Execution submission and provider creation need their own duplicate-safe
policy, not the same generic retry wrapper. Respect overload signals rather than
amplifying outages with synchronized retries.

**Regression.** Rotate the backend, send GOAWAY, drop connections, throttle calls,
and remove operation state. Verify platform properties reach placement and action
identity. Test capability mismatches before uploading a large input tree.

## 11. Maintenance And Diagnostics Are Part Of Execution

**Evidence.** [Buck #1842](https://github.com/facebook/buck/issues/1842) initially
looked like a deadlock, but maintainer analysis identified directory-cache scanning
and sorting during shutdown. Asynchronous cleanup still delayed returning to the
shell because shutdown waited for its executor. The discussion also warns that
unbounded post-command executors can accumulate across short commands.

[Buck #494](https://github.com/facebook/buck/issues/494) initially suggested
nondeterministic cache keys, but maintainers traced the key differences to different
Buck versions and client configuration. This was resolved as setup mismatch, not
a confirmed ordering bug. Useful key comparisons prevented chasing the wrong cause.

**Struo now.** Session logs and admission traces provide some context, and local VM
cleanup has bounded stages. They are not a complete phase-level liveness report or
persistent cleanup supervisor.

**Required safeguard.** Log sanitized identity differences, eligibility rejection,
queue wait, transfer, service queue, command execution, publication, materialization,
and cleanup separately. Record budget/domain and operation identifiers without
credentials or secret arguments. A stall report needs current phase, pending
capacity, last progress, and recovery attempts. Maintenance needs bounded work,
resumable progress, and explicit ownership; do not start an unbounded task pool
per invocation or hide whole-cache scans in foreground teardown.

**Regression.** Saturate admission, stall the server, and slow deletion separately;
each must produce distinguishable diagnostics. Fill the cache and run many short
commands; foreground exit latency and background task counts must stay bounded.

## Delivery Gates

These are follow-on requirements, not functionality already delivered by RFC 0014.
They should become integration tests alongside the subsystem that introduces them.

| Before enabling | Required evidence |
| --- | --- |
| Greater hosted fan-out | Capped or streamed response capture, byte/FD/staging budgets, admission wait diagnostics, and a starvation test |
| Cross-executor sharing | Immutable tools/environments, normalized execution paths, explicit equivalence, and cache trust partitioning |
| Remote graph execution | Executor eligibility, domain-aware graph fan-out, producer provenance, and bounded lost-input recovery |
| Native REAPI | Duplicate-safe action contract, resumable operations, canonical capabilities/platforms, proxy/rotation tests, and cancellation supervision |
| Shared transfers or workers | Subscriber-independent ownership and completion/cancellation race tests |
| Fallback or racing | Classified failure policy, independently eligible branches, isolated outputs, one publication owner, and loser lifecycle accounting |
| Hard resource guarantees | Separate requirement semantics, verified enforcement, and fail-closed tests on unavailable enforcement |

For the existing hosted adapters, prioritize bounded response capture and orphan
reconciliation before increasing default concurrency. For the next execution
milestone, prioritize immutable action environments and typed failure/recovery
semantics before remote graph placement. More executor strategies should follow
those contracts, not substitute for them.
