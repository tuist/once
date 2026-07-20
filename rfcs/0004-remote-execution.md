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

The Bazel Remote Execution protocol remains a future adapter over the same
prepared command contract.
