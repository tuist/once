# Remote Execution

Remote execution in Once means that the script contract stays in the
repository while the process runs in a fresh environment supplied by an
infrastructure provider.

The important boundary is the action, not the test framework. Vitest is the
program being run. Microsandbox, E2B, and Daytona are execution providers.

## What Moves To The Machine

For each remote action, Once performs this lifecycle:

1. Resolve the named provider and immutable image or template.
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

Use [Microsandbox](/guide/infrastructure/microsandbox) to test the contract on
a supported local computer. Use [E2B](/guide/infrastructure/e2b) when you have
a prepared hosted template. Use [Daytona](/guide/infrastructure/daytona) when
you want Once to start from a container image.

All three adapters implement the same declared input and output boundary. E2B
and Daytona use one archive for each transfer so large dependency trees do not
require one network request per file. The hosted image or template must
provide `tar` for unpacking and packing those archives.

## Machine Cleanup

Once waits for provider deletion after every normal success or failure. It
also schedules deletion if the executing task is cancelled while a Tokio
runtime is still active.

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
