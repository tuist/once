# Remote Execution

Remote execution in Once means that the script contract stays in the
repository while the process runs in a fresh environment supplied by an
infrastructure provider.

The important boundary is the action, not the test framework. Vitest is the
program being run. Docker, Microsandbox, E2B, and Daytona are execution providers.

Declared build graphs can also dispatch command actions with
`once build --remote --compute <provider>`. Once keeps dependency scheduling and
caching on the coordinator; each uncached command runs through the selected
adapter. Start with [Docker execution](/guide/infrastructure/docker) for local
validation, including the toolchain-matching requirements and current limits.
Graph filesystem operations remain on the coordinator; Docker applies command
directory setup inside the worker. Selecting a provider does not add a warm
worker pool or a distributed artifact service.

## What Moves To The Machine

For each remote action, Once performs this lifecycle:

1. Resolve the named provider and requested environment reference. Use an
   immutable image or template when reproducibility matters; a tag or label
   alone does not establish immutable identity.
2. Create a fresh sandbox.
3. Package only the declared input files, directories, executable modes, and
   safe symbolic links.
4. Transfer that package into a clean `/workspace` directory.
5. Run the declared argument list with its working directory, selected
   environment variables, resources, and timeout.
6. After a successful exit, retrieve only the declared outputs into a local
   staging area and validate them before installation.
7. Delete the sandbox, whether the action succeeded or failed.

Provider access keys configure the transport. They are not placed in the
action environment or included in the action identity.

## Run Real Vitest Tests

Treat dependency installation and testing as separate actions. The install
script uses the [npm package manager](https://www.npmjs.com/), consumes the
package manifests, and produces `node_modules`:

```sh
#!/bin/sh
# once input "../package.json"
# once input "../package-lock.json"
# once output "../node_modules"
# once cwd ".."
# once remote "remote_tests"

npm clean-install --no-audit --no-fund
```

The test script depends on that action and consumes its output:

```sh
#!/bin/sh
# once needs "./install.sh"
# once input "../src"
# once input "../tests"
# once output "../reports"
# once cwd ".."
# once remote "remote_tests"

./node_modules/.bin/vitest run \
  --reporter=json \
  --outputFile=reports/vitest.json
```

The provider environment supplies Node.js, npm, and basic operating-system
tools. The install action supplies the exact repository dependencies. Its
`node_modules` output becomes a declared input to the test action, including
package-manager symbolic links and executable modes.

This split means a package-lock change invalidates installation. A source or
test change invalidates the test action without forcing installation to run
again.

## Choose A Provider

Use [Docker](/guide/infrastructure/docker) to distribute graph command actions
across fresh containers with a locally prepared image. Use
[Microsandbox](/guide/infrastructure/microsandbox) to test the contract on
a supported local computer. Use [E2B](/guide/infrastructure/e2b) when you have
a prepared hosted template. Use [Daytona](/guide/infrastructure/daytona) when
you want Once to start from a container image.

These adapters implement the same declared input and output boundary. E2B
and Daytona use one archive for each transfer so large dependency trees do not
require one network request per file. The hosted image or template must
provide `tar` for unpacking and packing those archives.

## Executors And Resource Budgets

An executor describes how an action runs. Its resource domain describes the
capacity it consumes. Native processes and local Microsandbox virtual machines
share the host's CPU and memory budget, even though Microsandbox uses the same
prepared-command boundary as the hosted providers. Microsandbox guest memory
and CPU settings come from the admitted host reservation. The default memory
estimate is 250 MiB, so an undeclared guest no longer silently receives the
runtime's larger default allocation. Guest memory is rounded down to whole
mebibytes; a reservation below one mebibyte cannot start a guest.

E2B and Daytona use a separate shared quota for outstanding requests. All
hosted provider aliases share that quota, so adding another name does not add
capacity within that scheduler. Its default is the number of default-sized
local actions that fit in the host budget. These budgets belong to a shared
scheduler within one process, not a machine-wide or account-wide quota.
Separate CLI invocations do not share them. The service schedules its own physical machines; Once does not
reserve the remote guest's CPU or memory estimate on your computer.

A hosted request still reserves a fixed host-memory estimate for input
packaging, transfers, stream capture, and output validation. `--memory-limit`
controls admission against these estimates, not kernel-enforced process memory.
Hosted control responses, protocol frames, archive entries, and metadata have
explicit size limits. E2B streams command output to the cache with backpressure;
Daytona redirects the streams to temporary guest files and downloads them through
bounded local staging. Oversized or stalled transfers fail instead of growing
without a bound. Archive staging is shared by cloned schedulers, and outputs are
validated before publication. These limits bound the relevant buffers and staged
files, not the full process's resident memory or the remote guest's memory.
A quiet command is not a stalled transfer, including during setup. Transfer
inactivity defaults to a 30-second bound. Once the upload's file payload is
produced, waiting for the provider to drain and acknowledge it has a separate
five-minute bound. A declared action timeout bounds reconciliation and the whole
hosted work sequence. Cleanup runs outside that deadline with its own bounds.

Cache probes do not reserve action memory. Restoring cached outputs uses a
smaller, streaming-buffer estimate and does not need a host CPU slot or a remote
request slot. Named shared-resource slots are shared within an invocation
across resource domains for the entire action, including hosted execution.

A named infrastructure with `kind = "local"` selects native execution, including
its usual sandbox and network policies. It retains a separate cache identity
from an otherwise identical command without that binding. Provider names,
resource estimates, and environment bindings remain part of action identity;
Once does not yet claim that different providers run equivalent environments.

Sandbox-provider adapters currently cannot enforce `--network deny` or
stdout/stderr path redirection, or a non-default output-symlink policy. Once
rejects these requirements instead of
silently ignoring them. Selecting one executor never implicitly falls back to
another executor on a command or infrastructure failure. Explicit execution
account/project requirements are also rejected until an adapter can enforce
them; the configured access key selects the current hosted account.

Native Linux network denial requires Linux 5.11 or later, seccomp, and permission
to mark inherited descriptors close-on-exec. If enforcement is unavailable, the
action is refused. Plain Unix socket reads and writes remain available, but
`recvmsg`, descriptor passing, and batched messaging are blocked. Tools using
those IPC APIs can fail under network denial. An outside process can still proxy
requests over an allowed Unix socket; this is socket isolation, not proof that
no external service performed network work.

## Machine Cleanup

Once waits for provider deletion after every normal success or failure. It
also schedules deletion if the executing task is cancelled while a Tokio
runtime is still active. Execution-budget leases stay with that cleanup task
until the cleanup attempt completes, rather than being returned as soon as the
foreground task stops. Before creation, hosted actions persist an ownership
intent in workspace runtime state and tag the sandbox with a unique operation
marker. Creation has an owner independent of the caller, so a late response after
cancellation still registers cleanup. Failed deletion and uncertain creation
retain their records for bounded reconciliation on a later hosted action.
Reconciliation skips live owners and never deletes a sandbox without confirming
its current exact operation marker. HTTP refusal statuses do not establish that
creation had no side effects, so those intents remain journaled. An empty listing
under changed credentials cannot prove that an old sandbox is gone.

The journal admits at most 256 records and 576 directory entries before paid
creation. Unresolvable records consume that allowance and eventually refuse new
work rather than risking duplicate creation or deleting another owner's guest.
Provider lifetime policies remain necessary: a lost journal, credential rotation,
runtime shutdown, or rejected delete can still leave an orphan. A client quota is
not a physical fleet guarantee.

Microsandbox cleanup gives graceful stop up to ten seconds, then escalates to
kill and wait for up to five seconds. Removing a stopped guest's persisted state
has another ten-second limit. A kill timeout reports that the VM may still be
running and leaves its persisted state intact. Local VMs have no hosted lifetime
safety net, so an interrupted creation or failed kill may require manual cleanup.

The hosted providers add a second line of defense:

- E2B receives a finite lifetime and a kill-on-timeout policy.
- Daytona receives an ephemeral policy with immediate deletion after stop.

If the provider refuses a delete request after an otherwise successful action,
Once reports the action as failed. On an action failure, Once preserves the
original failure and records a cleanup failure in its internal log.

## How This Compares With Bazel

[Bazel remote execution](https://bazel.build/remote/rbe) asks which
content-addressed values are missing and uploads only those values. That is the
right transport for a large shared worker fleet because unchanged files do not
cross the network again.

The E2B and Daytona adapters currently upload one complete archive of the
declared inputs for each cache miss. This gives Once the same correctness
boundary, but it is not yet as bandwidth-efficient as Bazel. The provider
interface leaves room for a missing-content exchange so a future adapter can
deduplicate files without changing script annotations.

Once also keeps the executor boundary wider than the Bazel protocol. A local
micro-virtual-machine, a hosted sandbox service, and a content-addressed worker
fleet can all consume the same prepared action.
