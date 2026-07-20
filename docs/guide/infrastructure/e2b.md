# E2B

[E2B](https://e2b.dev/docs) provides hosted Linux sandboxes created from
templates. Once creates one sandbox for each action that misses the cache,
transfers declared inputs, runs the command, retrieves declared outputs, and
deletes the sandbox.

## Prepare A Template

Create an [E2B template](https://e2b.dev/docs/template) containing the stable
tools shared by your actions. A Vitest template should provide Node.js, npm,
and `tar`. Keep repository dependencies such as `node_modules` in the action
graph instead of baking them into the template.

Add the template name or identifier to the repository root `once.toml`:

```toml
[infrastructures.remote_tests]
kind = "e2b"
template = "vitest-node-22"

[infrastructure.execution]
provider = "remote_tests"
```

Pin the template contents so two executions with the same action identity see
the same tools.

## Authenticate

Set the provider access key outside the script:

```sh
export E2B_API_KEY=e2b_...
```

`ONCE_E2B_API_KEY` takes precedence when both variables are present. The key
authenticates sandbox creation, file transfer, and deletion. Once does not
forward it to the action process.

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

## Lifecycle And Network Access

Once creates E2B sandboxes with secure system communication, outbound network
access, a finite lifetime, and kill-on-timeout behavior. Outbound access lets
an installation action download packages. Declare downloaded dependencies as
outputs so later actions receive them explicitly.

Once sends an explicit delete request after success, command failure, timeout,
or transfer failure. The finite E2B lifetime remains a provider-side safety
net if the client disappears before it can delete the sandbox. E2B documents
the distinction between killed and paused sandboxes in its
[persistence guide](https://e2b.dev/docs/sandbox/persistence).

Self-hosted or test endpoints can override the control and sandbox addresses
with `ONCE_E2B_API_URL` and `ONCE_E2B_SANDBOX_URL`.
