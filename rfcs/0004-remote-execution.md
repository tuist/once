# Request for Comments 0004: Provider-Neutral Remote Execution

## Summary

Once should turn an annotated script or graph action into one provider-neutral
execution request, then let an executor adapter place that request on a local
Microsandbox, a hosted sandbox such as
[Daytona](https://www.daytona.io/) or [E2B](https://e2b.dev/), or a service
implementing Bazel's
[Remote Execution protocol](https://github.com/bazelbuild/remote-apis/blob/main/build/bazel/remote/execution/v2/remote_execution.proto).

The action contract owns the command, declared environment, working directory,
input tree, expected outputs, resources, timeout, and immutable execution
environment. Infrastructure configuration owns provider placement,
authentication, account, project, retry policy, and transport.

Scripts remain the migration path. They should gain remote execution by adding
headers, not by being rewritten as build graph rules.

## Decision

Once will preserve the immutable action model used by Bazel and Buck2 while
making the executor boundary broad enough for sandbox providers:

1. Resolve a script header or command option to a named infrastructure
   provider.
2. Prepare one normalized command with declared inputs and outputs.
3. Select an executor adapter from the provider definition.
4. Materialize only the declared input tree in a clean execution root.
5. Run the exact argument list without a host shell unless the script runtime
   itself is a shell.
6. Stream standard output and standard error while the command runs.
7. Retrieve only declared outputs after a successful exit.
8. Store the result and output trees in content-addressed storage.

An execution image or snapshot provides operating-system tools and language
runtimes. Repository source and generated dependencies are action inputs.

## What Bazel Gets Right

[Bazel remote execution](https://bazel.build/remote/rbe) represents the
command, input Merkle tree, and action as immutable content-addressed values.
The client asks which values are missing, uploads only those values, and asks a
remote service to execute the action digest. Results refer to output files and
trees by digest.

This is the correct network protocol for large shared fleets because it:

- avoids sending unchanged files repeatedly;
- makes upload work proportional to missing content;
- lets execution and caching share one action identity;
- permits independent workers to materialize the same action;
- records outputs without trusting a worker to choose undeclared paths.

Once should support this protocol natively rather than translating it through
a shell-based worker.

## What Buck2 Adds

[Buck2 remote execution](https://buck2.build/docs/users/remote_execution/) and
its [architecture](https://buck2.build/docs/concepts/architecture/) reinforce
three useful distinctions:

- analysis decides what the action means;
- execution platform selection decides where it may run;
- materialization decides when files must exist locally.

Once should keep these decisions separate. A coding harness often needs the
test result and captured streams, but it may not need every generated file
materialized immediately.

## Where Once Can Improve The Model

Bazel's protocol is an excellent distributed transport, but it should not be
the only executor interface. Daytona, E2B, and Microsandbox expose durable
sandboxes, snapshots, and filesystem operations rather than the Bazel service
shape. Once can normalize both families behind one prepared action:

```text
script or graph action
  -> semantic action and execution requirements
  -> named infrastructure binding
  -> executor adapter
     -> content-addressed remote service
     -> sandbox filesystem service
```

Sandbox adapters may initially transfer a declared tree directly. A mature
adapter should add content-addressed upload, missing-blob checks, cancellation,
leases, and capability discovery when the provider supports them.

Provider placement should eventually be excluded from semantic action
identity. Two providers executing the same command in the same immutable
environment should be able to share a result. The current implementation keeps
the provider binding in the action key as a conservative safety boundary until
environment and platform identity are mandatory and transport-independent.

## Worker Pools And Placement

Provisioning and execution have different ownership. A provider adapter creates
or claims capacity, checks readiness, renews leases, drains obsolete images, and
destroys resources. A prepared-command adapter stages inputs, starts a process,
observes it, cancels it, and retrieves verified outputs. A REAPI service owns
its fleet internally; Once should not require it to expose individual VMs.

Filter candidates for correctness before ranking them: execution OS and
architecture, ABI, immutable tools/SDKs, isolation and network requirements,
trust scope, resource capacity, and authorized artifact reachability. Rank only
eligible candidates by estimated finish time, including queue delay, startup,
missing-input transfer, and execution. Prefer locality for a bounded interval,
not forever. Each fallback or speculative attempt must pass the same predicate.

Warmth is not a single state. Retain immutable images, verified CAS contents,
host page caches, and explicitly namespaced compiler caches independently.
Every action receives a fresh writable workspace, temporary directory, and
home. Mutable caches must accelerate a correct cold execution, not supply
hidden dependencies. Persistent compiler processes need an explicit protocol,
compatibility key, reset boundary, and invalidation tests.

Pool capacity counts creating, idle, running, stopping, and uncertain workers.
Prewarming reserves resources before provisioning and keeps that reservation
while idle. A spare target is not a maximum pool size. Use exclusive worker
leases, bounded idle and total lifetimes, readiness probes, image rotation,
and crash reconciliation. Unknown cancellation or deletion outcomes remain
capacity and cost debt until termination is confirmed. A disconnected observer
must not cause blind resubmission or release a still-running worker's budget.

Keep output metadata, verified artifact availability, and local materialization
separate. Publish verified blobs before result metadata; successors should
eventually consume digests without downloading intermediates through the
coordinator. Missing-blob negotiation avoids repeat uploads. Across providers,
use authorized shared CAS access or bounded replication, accounting for egress,
region policy, retention, and provenance. A digest proves byte equality, not
authorization or an honest producer. Repair only after confirmed absence;
outages and corruption are not cache misses.

## Dagger And Stateful Macs

[Dagger's persistent engines](https://docs.dagger.io/self-hosting/) demonstrate
the value of engine affinity, persistent disks, incremental input transfer,
and explicit mutable cache volumes. That is not evidence that arbitrary
fine-grained actions can already be sharded across independent sandbox
providers. A compiler invocation remains opaque unless the graph decomposes
it into independently executable commands.

Reddit's [Mac RBE account](https://www.reddit.com/r/RedditEng/comments/1onkfoa/leveraging_bazel_multiplatform_rbe_for_reddits)
describes preserved Swift runner/module-cache state and controlled recycling.
This explains the reported statefulness without making a dirty checkout a
valid build input. Native macOS workers need pinned macOS, Xcode, SDK, and
architecture identities, clean action roots, and separate signing credentials.
Tart/Orchard or Anka can supply clean-image Macs; Docker cannot run a native
macOS guest. Apple cross-compilation inside Linux is a different capability.

## Research Constraints And Evaluation

The design borrows established results, not a claim of being faster than Bazel:

- [Build Systems à la Carte](https://www.microsoft.com/en-us/research/wp-content/uploads/2020/04/build-systems-jfp.pdf)
  separates dependency scheduling from rebuilding and cache validity.
- [Delay Scheduling](https://www.databricks.com/sites/default/files/2018/12/eurosys_delay_scheduling.pdf)
  motivates bounded locality waiting, but its Hadoop workload is not a compiler
  graph or a tiny heterogeneous Mac fleet.
- [Sparrow](https://people.eecs.berkeley.edu/~matei/papers/2013/sosp_sparrow.pdf)
  motivates readiness and late binding rather than trusting stale queues.
- [Omega](https://storage.googleapis.com/gweb-research2023-media/pubtools/3295.pdf)
  distinguishes placement observations from authoritative reservations.
- [Firecracker](https://www.usenix.org/system/files/nsdi20-paper-agache.pdf) and
  [REAP](https://doi.org/10.1145/3445814.3446714) distinguish fast provisioning,
  snapshot restoration, and working-set residency. Their serverless results
  do not establish compiler or macOS speedups.

Known failures supply acceptance cases: Buck2 fixed
[shared missing-blob cancellation](https://github.com/facebook/buck2/pull/1121),
and Bazel tracked a
[multiplex-worker cancellation leak](https://github.com/bazelbuild/bazel/issues/26292).
Cancellation, retention, and shared ownership need independent tests rather
than being inferred from successful builds.

Start with Docker for a free local per-action experiment, then compare a
prepared E2B template and Daytona's managed warm pools with exact resource
matching. For sustained fine-grained execution, evaluate a CAS-capable REAPI
backend such as BuildBarn, BuildBuddy, or EngFlow before inventing another
distributed artifact service. Modal is a possible later sandbox adapter;
Tart/Orchard and Anka address native Mac capacity. Tuist execution would be a
future adapter, not an existing consequence of its cache integration.

Measure uncached execution separately from result-cache hits. Record queueing,
provisioning, input bytes, execution, output transfer, cleanup, peak resources,
and total cost including idle capacity. Compare cold and warm outputs, evict
acceleration state, change inputs and environments, restart workers, and inject
lost responses, cancellation, and cleanup failures. Only measured workload
results can justify a claim of improvement over an existing backend.

## Node.js And Vitest Example

A [Vitest](https://vitest.dev/) workflow has two different dependency classes:

1. The immutable execution image supplies Node.js and basic operating-system
   tools.
2. An install action consumes package manifests and produces `node_modules`.
3. The test action consumes source files, test files, and the install action's
   declared output.

The test action never depends on the host's Node.js installation or ambient
workspace. An undeclared file must be absent in the execution root. Changing a
package lock invalidates the install action. If installation produces the same
tree, the test action retains the same dependency fingerprint.

This is better than putting `node_modules` into a long-lived provider snapshot.
Snapshots are effective for toolchains, but mutable repository dependencies in
a snapshot are difficult to inspect, key, share, and invalidate correctly.

## Hosted Sandbox Research

The first hosted adapters target E2B and Daytona because both expose the three
operations the prepared action requires: create an isolated machine, transfer
files, and destroy the machine.

- [E2B sandboxes](https://e2b.dev/docs) use prepared templates, a filesystem
  service, direct process arguments, and explicit kill semantics.
- [Daytona sandboxes](https://www.daytona.io/docs/en/sandboxes/) use snapshots
  or container-derived environments, a toolbox filesystem service, process
  execution, and ephemeral deletion policy.
- [Modal sandboxes](https://modal.com/docs/guide/sandboxes) also provide file
  transfer and termination, but their primary integration is through Modal
  language libraries. It remains a good candidate for a later adapter.
- [Cloudflare Sandbox](https://developers.cloudflare.com/sandbox/) exposes a
  capable sandbox inside the Cloudflare Workers object model. It is not a
  direct replacement for an executor called by a local Once process.

Provider popularity alone is not the contract. The deciding factor is whether
Once can own the complete machine lifecycle and preserve declared filesystem
boundaries without embedding a provider library in scripts.

## Hosted Transfer And Cleanup

E2B and Daytona receive one uncompressed archive containing only declared
inputs. This preserves directory structure, executable modes, and safe
symbolic links while avoiding one request per dependency file. A successful
action packages only declared outputs into a second archive. Once validates
that archive in a staging area before changing the workspace.

This transport sends the full declared tree on every action cache miss. It is
therefore less bandwidth-efficient than the missing-content exchange in the
Bazel protocol. It is an adapter implementation detail, so a provider can add
content-addressed transfer later without changing the prepared action or
script annotations.

Each hosted action owns a fresh machine. Once explicitly waits for deletion
after success and failure. E2B also receives a finite lifetime with
kill-on-timeout behavior. Daytona also receives an ephemeral policy with
immediate deletion after stop. These provider policies are safety nets for
client cancellation or process loss, not substitutes for explicit cleanup.

## Security Boundary

Declared paths are a security and correctness boundary, not only a cache hint.
An executor must reject:

- input or output paths that escape the workspace;
- absolute symbolic-link targets;
- relative symbolic links that escape every declared tree;
- special filesystem entries it cannot reproduce safely;
- provider output paths that were not requested.

Outputs should be downloaded into a staging area and installed only after all
requested trees have been validated. A failed installation must restore the
previous workspace outputs.

## Implemented Slice

The first slice establishes the provider-neutral prepared command and validates
it with a real local Microsandbox:

- named Microsandbox infrastructure with an explicit image;
- script annotations that resolve named infrastructure;
- direct transfer of declared files, directories, modes, and safe symbolic
  links;
- clean execution roots with logical runtime names;
- dependency outputs added to downstream action inputs;
- declared output retrieval with staged installation and rollback;
- a Node.js-backed workflow that installs and runs the real Vitest package;
- E2B template configuration, declared filesystem transfer, direct command
  execution, output retrieval, and deletion;
- Daytona image configuration, declared filesystem transfer, command
  execution, output retrieval, and deletion;
- provider-level tests that verify deletion after both successful execution
  and transfer failure.

The graph extension adds opt-in `once build --remote --compute <provider>` and
a Docker adapter with immutable local image IDs, caller-owned CPU/memory
admission, fresh per-command containers, declared filesystem setup, bounded
archive transfers, validated output installation, and journaled cleanup. Graph
analysis remains on a compatible coordinator, and provider binding partitions
configuration, outputs, and cache identity. File operations remain local.

This is not a running-worker pool or a remote CAS implementation. Intermediates
still return through the coordinator. Worker pooling, multi-provider placement,
native Mac workers, and the Bazel Remote Execution protocol remain future work
over the same prepared-command boundary.
