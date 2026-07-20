# Daytona

[Daytona](https://www.daytona.io/docs/en/sandboxes/) provides hosted sandboxes
with process and filesystem operations. Once creates one sandbox for each
action that misses the cache, transfers declared inputs, runs the command,
retrieves declared outputs, and deletes the sandbox.

## Configure An Image

Choose a Linux container image that contains the stable tools shared by your
actions. A Vitest workflow can use a pinned Node.js image:

```toml
[infrastructures.remote_tests]
kind = "daytona"
image = "node:22.18.0-bookworm"

[infrastructure.execution]
provider = "remote_tests"
```

Once asks Daytona to build the sandbox environment from that image. The image
must provide `tar`. Repository dependencies remain declared action outputs and
inputs, so changing a package lock invalidates the correct action.

## Authenticate

Set the provider access key outside the script:

```sh
export DAYTONA_API_KEY=...
```

`ONCE_DAYTONA_API_KEY` takes precedence when both variables are present. Once
does not forward the key to the action process.

## Run

Use the workspace default:

```sh
once exec --remote -- node --version
```

Or select the provider for one invocation:

```sh
once exec --remote --compute remote_tests -- node --version
```

An annotated script can select it with:

```sh
# once remote "remote_tests"
```

See [Remote Execution](/guide/infrastructure/remote-execution) for a two-action
Vitest example.

Daytona's command endpoint returns standard output and standard error as one
combined result. Once exposes that result as standard output.

## Lifecycle

Once marks each Daytona sandbox as ephemeral and sets immediate deletion after
stop. It also sends an explicit delete request after success, command failure,
timeout, or transfer failure. The ephemeral policy is a provider-side safety
net if the client disappears first. Daytona describes this behavior in its
[sandbox lifecycle guide](https://www.daytona.io/docs/en/sandboxes/#ephemeral-sandboxes).

The sandbox receives an automatic stop interval derived from the action
timeout, with extra time for provisioning and transfers. Actions without an
explicit timeout use Daytona's short-lived execution policy.

Self-hosted or test endpoints can override the control and toolbox addresses
with `ONCE_DAYTONA_CONTROL_URL` and `ONCE_DAYTONA_TOOLBOX_URL`.
