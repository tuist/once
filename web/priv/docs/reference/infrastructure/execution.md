# Execution Provider Contract

An execution provider runs one prepared command in an isolated environment.
Once owns graph analysis, action identity, scheduling, and caching. A provider
does not receive the entire build graph and does not need to implement target
kinds or test selection.

Start with the [provider guide](/guide/infrastructure/providers). This reference
describes the internal adapter boundary and hosted service requirements. It is
not an HTTP schema or a claim that Once supports REAPI `Execution.Execute`.

## Prepared Command

The internal [`PreparedCommand`](https://github.com/tuist/once/blob/main/crates/once-core/src/remote.rs)
passes these values to an execution adapter:

- **`argv`:** argument vector with a program and separate arguments. Preserve
  boundaries, including spaces, quotes, empty arguments, and metacharacters.
- **`env`:** selected environment entries. Do not forward the caller's whole
  environment or provider credentials.
- **`cwd`:** optional workspace-relative directory. Existing sandbox adapters
  resolve it beneath `/workspace`; absent means the workspace root.
- **`inputs`:** declared workspace-relative paths, including dependency
  outputs. Transfer only this boundary, not the entire checkout.
- **`outputs`:** declared workspace-relative paths to retrieve after an
  accepted exit code.
- **`resources`:** declared resource request used for admission. A request is
  not proof of guest-side enforcement.
- **`timeout_ms`:** optional action deadline. Hosted setup, reconciliation,
  transfers, and execution must fit within the action budget; cleanup has
  separate bounds.
- **`success_exit_codes`:** accepted command exit codes, normally `[0]`. A
  nonzero accepted code can still produce valid outputs.
- **`lease`:** owned admission permit. Keep it alive while execution,
  staging, capture, or cleanup still consumes resources.

Graph callers can also pass explicit `clean_paths` and `create_dirs` through
the admitted command runner. Docker applies this setup inside its worker after
staging ordinary inputs, then restores explicitly selected post-setup inputs,
such as response files inside reset scratch directories. This does not import
coordinator scratch contents implicitly. Other remote adapters currently reject
nonempty setup. Setup must participate in the graph caller's command/cache
identity; passing it separately does not key it
automatically. Portable graph filesystem operations still run on the coordinator.

Race-safe archive transfer currently supports Linux and Apple coordinators.
Other coordinator platforms return an unsupported-platform error rather than
falling back to unsafe path traversal. This applies to all archive-backed
remote adapters, not only Docker.

The resolved [`RemoteExecution`](https://github.com/tuist/once/blob/main/crates/once-core/src/action.rs)
separately identifies the named provider, executor adapter, environment
reference, and optional account/project requirements. Those requirements must
be enforced or rejected, never silently ignored.

Use Once's
[`resolve_execution_argv` and `resolve_execution_env`](https://github.com/tuist/once/blob/main/crates/once-core/src/execution_path.rs)
to resolve workspace placeholders at the guest root. Preserve argv semantics
even if a provider API only accepts a command string. E2B's process transport
and Daytona's command quoting are examples, not a reason to require a shell
for every action.

## Service Operations

There is no universal wire API for these operations today. A new adapter maps
its service's endpoints onto this lifecycle:

1. **Create.** The service allocates a fresh isolated environment from the
   requested image/template and returns a stable machine id. The adapter
   authenticates, journals ownership before the request, attaches the operation
   marker, and registers cleanup when the id arrives.
2. **Find owned machines.** The service returns ids and exact ownership
   metadata and makes truncation/pagination visible. The adapter reconciles
   uncertain creation or deletion without touching another operation's machines.
3. **Transfer inputs.** The service accepts a bounded file/archive transfer
   without publishing incomplete content as ready. The adapter packages
   declared inputs, preserves modes and safe links, and unpacks into a clean
   execution root.
4. **Execute.** The service runs the command with its cwd/environment, returns
   exit status, and exposes stdout/stderr. The adapter preserves argument
   semantics, applies the deadline, and captures streams with backpressure or
   bounded staging.
5. **Retrieve outputs.** The service returns the declared files/directories
   after an accepted exit. The adapter validates and stages all returned
   content before installation in the local workspace.
6. **Delete.** The service terminates and removes the machine; already missing
   counts as deleted. The adapter awaits bounded cleanup on success and
   failure, retains uncertain records, and releases resources only when their
   work ends.

Do not install Once on the worker just to reconstruct the graph. The worker
needs the action's tools and transfer/process facilities. The current hosted
archive adapters also require `tar` and their setup utilities in the guest.
Those are requirements of these transports, not a mandatory wire format for
all future providers.

Use the [E2B adapter](https://github.com/tuist/once/blob/main/crates/once-core/src/remote/e2b.rs)
as a hosted reference for preparation, command execution, output retrieval,
and cleanup. The
[Daytona adapter](https://github.com/tuist/once/blob/main/crates/once-core/src/remote/daytona.rs)
shows a service that captures streams in guest files instead of returning them
as a live process stream.

## Input and Output Safety

Inputs may include files, directories, executable modes, and safe symbolic
links, such as package-manager executable links. Do not flatten directories,
dereference arbitrary links, or upload undeclared host paths. See the
[input archive planner and builder](https://github.com/tuist/once/tree/main/crates/once-core/src/remote/archive/input)
and [path validation](https://github.com/tuist/once/blob/main/crates/once-core/src/remote/path.rs).

On an accepted exit, retrieve only declared outputs. A missing declared output
is a failure. Reject traversal, absolute paths, undeclared entries, unsafe
links, and unsupported archive entry types. Do not extract remote archives
directly into the customer's workspace. Stage and validate first, then install
with rollback if a multi-output installation fails. Reuse the
[output archive validator](https://github.com/tuist/once/blob/main/crates/once-core/src/remote/archive/output.rs)
and [`OutputStaging`](https://github.com/tuist/once/blob/main/crates/once-core/src/remote/output_install.rs).

On an unaccepted exit, return the original exit status and captured streams;
do not publish successful outputs. Separate command failure from transport,
policy, validation, timeout, and cleanup failures. The existing adapters return
an [`ActionResult`](https://github.com/tuist/once/blob/main/crates/once-cas/src/model.rs)
for the status and stream digests; the surrounding Once runner captures
installed output values and publishes cache results.

## Environment and Policy Enforcement

- The environment must supply the declared tools and compatible OS/architecture.
  Use an immutable image or template reference for reproducibility. Existing
  adapters do not prove immutable identity from a mutable label.
- Provider credentials authenticate control and transfer requests. They are
  not action inputs and are not forwarded to the process.
- Validate requirements before paid creation. Existing sandbox adapters reject
  network denial, stdout/stderr path redirection, non-default output-symlink
  policies, and explicit execution account/project scope because they cannot
  enforce those requirements.
- A new adapter must either enforce a policy and demonstrate it in tests, or
  keep the same explicit refusal. Never downgrade network denial or tenant
  isolation to best-effort behavior.
- Executor failure must not implicitly fall back to another executor. Cache
  unavailability policy is separate from execution placement.

[`Executor::resolve`](https://github.com/tuist/once/blob/main/crates/once-core/src/executor/selection.rs)
owns current policy checks. Adding a service includes wiring its selection and
resource domain, not just adding an HTTP client.

## Resource and Transfer Budgets

Local processes and local virtual machines consume the host resource domain.
Docker additionally reserves against the actual daemon's CPU capacity and two
thirds of its reported memory, shared across aliases within the process.
Container limits follow that worker reservation, which survives cancellation
until physical cleanup completes or uncertain ownership is reconciled.
Hosted providers consume the remote request domain and still reserve bounded
host memory for packaging, transfers, capture, and validation. Aliases share
the scheduler's remote request quota; another name does not create more
capacity. The service must enforce physical machine and tenant quotas itself.

Use [`ExecutorResources` and `ExecutionPermit`](https://github.com/tuist/once/blob/main/crates/once-core/src/executor/admission.rs)
and the shared staging/capture pools. Do not create an unbounded transfer or
machine fan-out outside admission. Keep permits alive through background work
and cancellation cleanup.

[`RemoteIoLimits`](https://github.com/tuist/once/blob/main/crates/once-core/src/executor/io.rs)
bounds response/frame bytes, archives, entries, metadata, capture, staging,
and transfer progress. Stream large data through bounded buffers and temporary
files. These client limits are not guest memory enforcement or a bound on the
whole client's resident memory.

Output publication shares pinned directory handles across files with the same
parent. At most 128 output-parent handles are retained across installations in
one process. Exceeding that budget fails with a resource-limit error and rolls
back published outputs rather than exhausting file descriptors. Declaring a
directory output instead of many independently rooted files reduces this cost.
Archive extraction charges implicit ancestor directories against entry and
metadata limits before creating them. Archived output paths are limited to 64
components to bound validation and cleanup depth.

A quiet command is not a stalled transfer. Transfer inactivity defaults to
30 seconds; after an upload's body ends, acknowledgment has a separate
five-minute bound. A declared action timeout bounds the foreground sequence.
Cleanup runs outside that deadline with its own time limits, even if the
foreground action has already timed out.

## Ownership, Cancellation, and Cleanup

The shared [hosted lifecycle](https://github.com/tuist/once/blob/main/crates/once-core/src/remote/lifecycle.rs)
provides the ownership journal, creation ownership, reconciliation, and
deletion retries. Integrate it instead of relying only on a `finally` block.

1. Persist a creation intent before sending the create request. Include the
   unique `once-operation` marker in provider metadata.
2. Create on an independently owned task. If cancellation wins the race, a
   late create response must still register and run cleanup.
3. Persist the machine id when known. Do not blindly retry creation: an error
   or timeout does not prove that no paid machine was created.
4. Await deletion on normal success and failure. On cancellation while the
   runtime is active, keep the execution permit with the cleanup task.
5. Retain uncertain creation and failed deletion records for bounded
   reconciliation on a later action. Never delete by a guessed id or a label
   without checking the current exact operation marker.

The lifecycle's internal `Provider` trait supplies `scope`, `credential`,
`find`, and `delete`. `HostedSandbox` supplies the id. These are cleanup seams,
not a complete execution-provider trait: transfer and process calls still
belong to the adapter. See the
[E2B cleanup binding](https://github.com/tuist/once/blob/main/crates/once-core/src/remote/e2b/cleanup.rs)
for a concrete implementation.

Authenticated lookup must distinguish a complete absence from an incomplete
or untrusted listing. Another credential's empty listing cannot establish
that an old machine is gone. Reconciliation skips live owners and never
deletes a foreign operation's machine.

The journal is bounded and refuses new creation when its allowance is full.
It is not a guarantee against all orphans: lost state, revoked credentials,
runtime termination, or a provider refusing deletion can still leave machines.
Supply a provider-side finite lifetime or equivalent automatic termination
policy and an operator recovery procedure.

If cleanup fails after a successful command, report failure rather than a
clean success. If both command execution and cleanup fail, preserve the
original failure and log the cleanup failure without secrets.

## Acceptance Checks

Use disposable machines and a fault-injecting transport, then repeat the
essential scenarios against your real service. Force cache misses so a cache
hit cannot bypass the executor being tested.

1. **Dependency tree with executable files and safe links:** exact declared
   input tree and modes available at the requested cwd. See the
   [input archive tests](https://github.com/tuist/once/blob/main/crates/once-core/src/remote/archive/input/tests.rs).
2. **Arguments containing spaces, quotes, empty strings, and metacharacters:**
   same argv and selected environment, without accidental shell evaluation.
   See the [Daytona quoting tests](https://github.com/tuist/once/blob/main/crates/once-core/src/remote/daytona.rs)
   and [E2B process transport](https://github.com/tuist/once/blob/main/crates/once-core/src/remote/e2b/protocol/process.rs).
3. **Accepted nonzero exit versus unaccepted exit:** outputs returned only for
   accepted codes, preserving status and streams. See
   [`PreparedCommand` acceptance and adapters](https://github.com/tuist/once/blob/main/crates/once-core/src/remote.rs).
4. **Missing, malicious, or partially installable outputs:** failure without
   unsafe publication; failed installation rolls back earlier outputs. See
   the [output validation tests](https://github.com/tuist/once/blob/main/crates/once-core/src/remote/archive/output.rs)
   and [rollback tests](https://github.com/tuist/once/blob/main/crates/once-core/src/remote/output_install/tests.rs).
5. **Oversized/stalled transfer or excessive capture:** bounded failure and
   cleanup, without unbounded memory or staging growth. See the
   [E2B transport tests](https://github.com/tuist/once/blob/main/crates/once-core/src/remote/e2b/transport/tests.rs).
6. **Upload, startup, execution, deadline, or deletion failure:** cleanup is
   attempted and awaited without hiding the original failure. See the
   [E2B lifecycle tests](https://github.com/tuist/once/blob/main/crates/once-core/src/remote/e2b.rs)
   and [Daytona lifecycle tests](https://github.com/tuist/once/blob/main/crates/once-core/src/remote/daytona.rs).
7. **Lost create response, cancellation, credential rotation, or foreign marker:**
   no duplicate create or foreign deletion; ownership records retained and
   permits held through cleanup. See the
   [hosted ownership tests](https://github.com/tuist/once/blob/main/crates/once-core/src/remote/lifecycle/tests.rs).
8. **Unsupported policy or tenant requirement:** refusal before allocation,
   never silent weakening. See the
   [executor selection tests](https://github.com/tuist/once/blob/main/crates/once-core/src/executor/selection.rs).

These references are examples to extend for your adapter, not a claim of
service certification. Inspect the provider's inventory after failure and
cancellation scenarios and verify that the service's lifetime safety net
handles client disappearance. See the [handoff checklist](/guide/infrastructure/providers#handoff-checklist)
before enabling customers.
