---
prev: false
---

# Infrastructure

Once works locally without configuration. Infrastructure becomes useful when
you want to run an action on another machine or share a cached result with
another developer or coding agent.

## Run An Action Somewhere Else

Choose a named execution provider in the repository root `once.toml`:

```toml
[infrastructures.remote_tests]
kind = "e2b"
template = "vitest-node-22"

[infrastructure.execution]
provider = "remote_tests"
```

Then run a literal command through that provider:

```sh
once exec --remote -- node --version
```

Annotated scripts can select the same provider with
`# once remote "remote_tests"`. Once creates a clean execution root, transfers
only declared inputs, runs the command, retrieves declared outputs after a
successful exit, and deletes the machine.

Read [Remote Execution](/guide/infrastructure/remote-execution) for the full
action lifecycle and a real Vitest workflow.

## Share Results Across Machines

A cache provider lets another machine reuse an action result instead of
running it again:

```toml
[infrastructures.tuist]
kind = "tuist"
account = "acme"
project = "app"

[infrastructure.cache]
provider = "tuist"
```

The execution and cache capabilities are independent. A repository can run
actions through E2B or Daytona while storing reusable results through Tuist.
It can also use either capability on its own.

## Available Providers

| Provider | Capability | Environment | Best fit |
| --- | --- | --- | --- |
| [Microsandbox](/guide/infrastructure/microsandbox) | Execution | Local image | Validate an action boundary on the current computer. |
| [E2B](/guide/infrastructure/e2b) | Execution | Hosted template | Start a prepared hosted environment quickly. |
| [Daytona](/guide/infrastructure/daytona) | Execution | Hosted container image | Build a hosted environment from a familiar image. |
| [Tuist](/guide/infrastructure/tuist) | Shared cache | Not applicable | Reuse action results across machines. |

## Keep A Local Escape Hatch

The local cache remains available after a shared cache is configured. Set
`ONCE_CACHE_PROVIDER=local` for one invocation:

```sh
ONCE_CACHE_PROVIDER=local ./scripts/greet.sh
```

For execution, omit `--remote` to run on the current computer. Pass
`--compute <provider>` with `--remote` to choose a different named execution
provider for one invocation.

## Next

Start with [Remote Execution](/guide/infrastructure/remote-execution), then
open the setup guide for the provider you want to use.
