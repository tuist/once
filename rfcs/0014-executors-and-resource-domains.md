# Request for Comments 0014: Executors And Resource Domains

## Motivation

Struo needs an explicit execution boundary before adding a content-addressed
remote worker service. A provider selects infrastructure, but it does not tell
the scheduler which physical resources an action consumes. In particular, a
local micro-VM and a native process compete for the same host, while a hosted
sandbox does not consume the host CPU and memory requested for its guest.

The proposal extends [provider-neutral remote execution](0004-remote-execution.md).
It does not replace the prepared command, declared filesystem boundary, or
script annotations.

## Research: Bazel

Bazel separates an action's command and declared inputs from the strategy that
executes its spawn. Strategies include standalone processes, sandboxes,
persistent workers, and remote execution. Execution platforms describe where
build tools can run; target platforms describe what the build produces. Those
are different even without remote execution, for example when cross-compiling.

The [Remote Execution API](https://github.com/bazelbuild/remote-apis/blob/main/build/bazel/remote/execution/v2/remote_execution.proto)
represents a command, input tree, and action as immutable CAS objects. Its
`Action.platform` contains requirements that any selected worker must satisfy.
The service chooses a worker. A client does not need to inventory physical
machines to submit an action. Execution priority lives in `ExecutionPolicy`,
outside the action. Platform requirements and execution timeout remain inside
the action because they affect the validity of a reusable result.

Bazel's [ResourceManager](https://github.com/bazelbuild/bazel/blob/master/src/main/java/com/google/devtools/build/lib/actions/ResourceManager.java)
admits local work against CPU, memory, named resources, and worker availability.
It is not an inventory of the remote fleet. Its implementation has separate
local and dynamic request priorities, and releases resources through handles.
This protects the build client independently of the remote service's scheduler.

[Dynamic execution](https://bazel.build/remote/dynamic) runs local and remote
branches concurrently, cancels the loser, and publishes only the winning
outputs. It requires compatible local and remote environments and output
ownership coordination. It is useful because remote parallelism improves clean
builds, while local execution can avoid network overhead on short incremental
actions. It also spends extra resources and can delay more useful local work.
The documented default is to fail if the first completed branch fails, rather
than hide divergent local and remote behavior.

## Research: Buck2

Buck2 [resolves execution platforms during configuration](https://buck2.build/docs/rule_authors/configurations/).
It selects the first registered platform satisfying the target's execution
constraints and the compatibility of its execution dependencies. Tools are
configured for the execution platform, not the target platform. The platform
also supplies the executor configuration and remote scheduling properties.

[CommandExecutorConfig](https://github.com/facebook/buck2/blob/main/app/buck2_build_api/src/interpreter/rule_defs/command_executor_config.rs)
separates local enablement, remote enablement, remote properties, cache policy,
and hybrid policy. The [executor types](https://github.com/facebook/buck2/blob/main/app/buck2_core/src/execution_types/executor_config.rs)
explicitly support local execution with a remote cache. Remote caching and
remote computing are not the same capability.

The [hybrid executor](https://github.com/facebook/buck2/blob/main/app/buck2_execute_impl/src/executors/hybrid.rs)
composes local and remote executors. Limited execution, fallback, and full
racing are distinct policies. Fallback after a command failure requires explicit
permission; timeouts do not normally trigger fallback. It also controls output
claims and cancellation. The [local executor](https://github.com/facebook/buck2/blob/main/app/buck2_execute_impl/src/executors/local.rs)
uses a host-sharing broker and local-resource holders. Shared local resources
are acquired in a stable order to avoid deadlock.

Buck2's [remote client configuration](https://buck2.build/docs/users/remote_execution/)
identifies execution, CAS, and action-cache services independently. It sends
platform requirements to the remote service, which controls worker placement.
[Deferred materialization](https://buck2.build/docs/users/advanced/deferred_materialization/)
separates a result's availability in CAS from the need to put it on the host.
That is important when downstream consumers also execute remotely.

These are architectural reasons to introduce executors, not reasons to copy
Bazel's flags or Buck2's configuration syntax into Struo.

## Research: Observed Failure Modes

The [failure-analysis companion](0014-executor-failure-modes.md) examines public
Bazel, Buck, and Buck2 issue discussions and fixes, distinguishing reported
problems from confirmed defects, deliberate tradeoffs, and resolved setup errors.

The strongest lessons concern subsystem boundaries:

- Undeclared tools can produce incorrect cache hits, while client-host details
  can fragment equivalent remote actions. Placement alone proves neither safety
  nor equivalence.
- Dynamic execution can start an ineligible local branch. Each candidate must
  satisfy requirements independently, including during fallback.
- CPU-oriented fan-out throttles remote work, but removing it without byte,
  file-descriptor, staging, and nested-tool budgets can overwhelm the client.
- Cancellation of a deduplication owner can fail unrelated waiters; a late worker
  response can leak slots. Shared work needs subscriber-independent ownership.
- Transfer limits can deadlock their completion path, and HTTP/2 flow control can
  stall despite application admission. Nonblocking limits need liveness tests.
- An action-cache hit does not prove referenced blobs still exist. Transient CAS
  outages must not be turned into missing inputs, and repair needs bounded producer
  recovery plus circuit-breaker recovery.
- REAPI explicitly permits duplicate execution and does not require cancellation
  support. An operation completing or a client stream closing does not prove all
  work stopped.
- Daemon incrementality, durable cache reuse, and local materialization are
  different states. Restart and eviction tests must cover them independently.

The current boundary addresses shared host accounting, hosted request admission,
and cleanup leases. It does not yet satisfy the companion's delivery gates for
bounded hosted capture, immutable environments, remote graph provenance, native
REAPI interoperability, or speculative execution. Those are prerequisites for
expansion, not benefits implied by introducing the executor type.

## Comparison With Struo

| Concern | Bazel / Buck2 | Struo today |
| --- | --- | --- |
| Semantic action | Command, inputs, outputs, environment and platform requirements | Typed actions and a provider-neutral prepared command |
| Placement | Spawn strategy or execution-platform executor configuration | Optional `RemoteExecution` and adapter dispatch |
| Cache transport | Separate from execution strategy | Separate cache provider, including local execution with remote caching |
| Host admission | Local resource manager or host-sharing broker | One `ResourcePool` around every cache miss |
| Remote admission | Client request limits plus server worker scheduling | Remote requests currently consume full host action estimates |
| Local virtual machines | Local resource consumers | Microsandbox uses the remote command boundary but computes locally |
| Graph placement | Configured tools and execution platform | Graph actions currently lower to local execution |
| Materialization | Independent policy, including deferred outputs | Graph outcomes can defer restoring cached outputs; hosted actions install outputs locally |
| Hybrid execution | Explicit policy, cancellation and output claims | No racing or fallback |

Struo's immediate gap is resource ownership, not a missing fleet scheduler. A
large remotely executed action must not reserve its guest memory on the client,
and a local sandbox must not get a second independent host budget.

## Proposed Model

An **action** describes reproducible work. An **execution platform** describes
the tool and environment compatibility required by that work. An **executor**
implements a way to run it. A **resource domain** describes shared capacity and
owns admission. **Placement policy** selects an eligible executor. **Materialization
policy** decides when outputs must exist locally.

```text
action + execution requirements
  -> eligible executor
  -> resource-domain admission
  -> prepared command execution
  -> validated result in CAS
  -> output materialization
```

Bind executors to resource domains, not necessarily individual machines:

- Native execution and local virtual machines share the host domain.
- A remote service is a domain representing outstanding client requests or an
  account's quota. Its server owns physical worker CPU and memory admission.
- Multiple aliases for the same capacity must share a domain. An executor name
  must not manufacture capacity.
- A future directly managed worker may expose its physical budget as a domain,
  without forcing hosted services to reveal their fleet topology.

Keep remote request admission distinct from host-side client work. Input
packaging, network buffers, stream capture, output validation, and installation
still need bounded host resources. A remote action's guest resource estimate is
sent to the backend where supported; it is not a host reservation.

Admission is a shared lease. A sandbox cleanup task retains it after the
foreground future is cancelled and releases it after the cleanup attempt
completes. Acquiring more than one domain must follow one documented order;
a request must not hold host capacity while waiting for a remote quota. Initial
cache output restoration requires memory-only host materialization admission,
but probes do not reserve action memory and initial hits do not occupy
a worker or remote request slot. A cache recheck after admission can discover a
result that became available while the action waited.

A client quota bounds admitted operations and cleanup attempts, not unobserved
machines. If a create response is lost, a delete fails, the runtime shuts down,
or a cleanup request times out, the client cannot prove the machine stopped.
Provider lifetimes and reconciliation remain necessary. Native host estimates
and VM overhead are also estimates, not kernel-enforced memory limits.

## Identity And Failure Policy

This change must preserve the current action encoding and digests. Domain
capacity and admission policy are runtime state, not new action fields. Struo
currently keys resource estimates and provider bindings conservatively. Do not
remove those fields merely because Bazel separates transport from identity.

Cross-executor result sharing requires a transport-independent, immutable
environment identity, platform compatibility, declared tool identities, and a
trusted cache namespace. Mutable image tags, provider default environments,
and ambient host tools do not establish that equivalence.

The initial policy selects exactly one executor and fails closed on an unknown
adapter. Do not fall back to a host process when authentication, image startup,
transport, or a remote command fails. Retrying a non-idempotent action after a
lost response can duplicate effects. Racing additionally requires isolated
outputs, a publication claim, cancellation acknowledgement, and accounting for
both branches until they stop. Those are separate work, not implicit behavior
of this boundary.

## Delivery

### Implemented Boundary

Use a closed, typed executor descriptor for the built-in adapters, not an async
trait with a premature third-party extension contract. Native and Microsandbox
execution select the host domain. E2B and Daytona select a single shared remote
request domain. Even aliases for different hosted services share this initial
conservative client quota. Per-service or per-account domains are future work.

`ExecutorResources` owns both budgets. Its default remote request limit is the
host pool's bounded default-action parallelism, preserving the previous default
fan-out. The runtime API can override remote request concurrency independently
of host CPU capacity and set a fixed client memory estimate. By default client
work uses the existing host default estimate. It reserves no guest CPU slots.
Host-wide named slots are charged atomically with client memory and held for
the entire request. Admission order is remote quota, then host memory and slots.

`Runner` uses this boundary for captured, streaming, fresh, and reproducibility
execution. Cache probes run without a worker lease; output restoration reserves
a small fixed streaming-buffer estimate instead of the default action estimate.
Its clones share both domains; independently constructed runners and separate
CLI processes do not. These are invocation-scoped budgets, not machine-wide
or account-wide quotas. Convenience
cache-aware calls create per-call pools, so concurrent schedulers must reuse a
runner or supply shared executor resources. Raw uncached entry points remain
caller-admitted for graph execution and other explicitly budgeted callers.
The public cache SDK does not grow an execution API.

Microsandbox is explicitly sized from the admitted CPU and memory reservation,
not its runtime's implicit defaults. Undeclared guests use the generic default
action estimate, which currently lowers their allocation from 512 MiB to 250
MiB. VM memory rounds down to its mebibyte granularity; a nonzero admitted budget
below that granularity fails before creation. VM overhead remains a separate
estimate to add later. Cleanup bounds graceful stop to ten seconds, kill and
wait to five seconds, and persisted-state removal to ten seconds. A kill timeout
returns an error and retains persisted state for manual recovery; it cannot
prove that the guest stopped. An interrupted VM creation can still leave a
local orphan without a provider lifetime safety net.

A `local` infrastructure binding now selects the native executor and honors
native sandboxing, stream redirects, and network policy. Adapter selection
rejects unknown executors and requirements the sandbox adapters cannot honor:
network denial, stdout/stderr path redirects, and non-default output-symlink
policies. Unsupported output observers
and filesystem contract auditing also fail instead of silently doing less than
requested. This is a deliberate tightening of previously ignored requirements.

Keep scripts as the remote migration ramp and keep graph execution local until
its toolchain and environment resolution can produce remote-compatible actions.
Do not add a graph remote flag that moves host-resolved executable paths onto
an incompatible worker. There are no new manifest settings in this slice;
existing infrastructure selection and action encodings remain unchanged.

### Hardening The Boundary

The [hardening implementation](0014-executor-hardening.md) adds fair admission,
required-minimum validation, bounded hosted capture and staging, cancellation-owned
blocking publication, durable hosted lifecycle reconciliation, typed cache errors
and recovery, native process-group supervision, and deferred-output producer
repair. Runtime I/O policies do not change action encoding or digest identity.
Native network denial fails closed, including loopback; uncacheable graph actions
must not bypass it. Explicit account/project placement is rejected until enforced.

### Follow-On Work

- Resolve immutable platform and toolchain identities before changing result
  sharing, tenancy namespaces, or resource fields in the action digest.
- Add per-service/account remote domains and domain bindings through `struo.toml`,
  not a new manifest or a provider-specific graph surface.
- Add resource declarations and generic execution requirements to target kinds,
  along with the matching public Starlark reference and validation.
- Decouple graph target fan-out, action batching, event worker slots, and test
  batch processes from host-only capacity when graph remote placement exists.
- Add VM-overhead estimates and phase-specific transfer CPU admission. Hosted
  capture and staging now have explicit limits, but the client reservation is
  still an admission estimate, not a hard total resident-memory guarantee.
- Add a native REAPI adapter with missing-blob exchange, resumable server operation
  tracking, optional-cancellation capability handling, and bounded recovery of
  lost deferred input blobs. Restrict this path to duplicate-safe actions because
  the protocol does not guarantee at-most-struo execution.
- Distinguish verified blob absence, transient transport failure, corruption,
  command failure, and uncertain execution state. Recovery budgets and circuit
  breakers must preserve these categories.
- Give shared transfers and future worker requests subscriber-independent task
  ownership, with completion/cancellation race tests. Add transfer liveness and
  service-rotation tests through a proxy and independent REAPI implementations.
- Make remote materialization policy explicit before keeping intermediate
  artifacts remote, including producer provenance and cache-location identity.
- Separate consumption estimates from minimum capacity and required enforcement;
  normalization of an estimate must never satisfy an unmet hard requirement.
- Add sanitized phase-level queue, transfer, execution, materialization, and
  cleanup diagnostics, including uncertain cleanup outcomes.
- Add fallback or dynamic execution only with classified failures, independently
  eligible branches, compatible platform identities, isolated outputs, and
  publication claims. Account for losing work until its terminal state is known.

## Validation Requirements

- Native and local virtual-machine executors compete for the same host budget.
- Remote guest estimates do not consume host CPU or guest-sized host memory.
- Remote client admission respects the host memory budget. This does not assert
  a hard total resident-memory bound; bounded protocol and capture buffers have
  separate fault tests.
- Aliases cannot multiply a shared domain's capacity.
- Success and failure return leases after cleanup; cancellation retains them
  through detached deletion, including cancellation while awaiting deletion.
- Cache hits restore under host admission without acquiring remote capacity.
- Unknown adapters fail before sandbox creation and never run locally.
- Existing action digests, script headers, and graph local behavior remain stable.
- Tests exercise both ordinary and streaming execution entry points.

The [failure-analysis delivery gates](0014-executor-failure-modes.md#delivery-gates)
add subsystem-specific fault-injection requirements before increasing hosted
fan-out, enabling cross-executor sharing or remote graph placement, introducing
REAPI, or adding shared workers and speculative policies. These future gates are
not claims that the current implementation has already passed those tests.
