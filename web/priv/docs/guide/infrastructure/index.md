---
prev: false
---

# Infrastructure

Struo works locally without configuration. Infrastructure becomes useful when
you want to run an action on another machine or share a cached result with
another developer or coding agent.

[Artifacts](/guide/infrastructure/artifacts) describes a third
independent capability: retain immutable build products for distribution.
Struo publishes the content; infrastructure decides how people use it.

## Build an Integration

If you operate an infrastructure service, start with
[Become an Infrastructure Provider](/guide/infrastructure/providers). It
walks through shared caching, the adapter work required in Struo,
authentication, validation, and the customer handoff. The
[provider contracts](/reference/infrastructure/) give exact behavior and links
to the implementations and tests.

## Create And Connect A Project

Provision a remote project and bind this repository to it in one command:

```sh
struo connect --provider tuist --create
```

Struo signs you in when needed, creates the project with the provider, and
records the binding in the repository root `struo.toml`. Read
[Connect A Project](/guide/infrastructure/connect) for account selection,
binding an existing project, and previewing the write.

## Run An Action Somewhere Else

Choose a named execution provider in the repository root `struo.toml`:

```toml
[infrastructures.remote_tests]
kind = "e2b"
template = "vitest-node-22"

[infrastructure.execution]
provider = "remote_tests"
```

Then run a literal command through that provider:

```sh
struo exec --remote -- node --version
```

Annotated scripts can select the same provider with
`# struo remote "remote_tests"`. Struo creates a clean execution root, transfers
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

A bucket you already have works as well; see
[Object Storage Cache](/guide/infrastructure/object-storage). So does an HTTP
build cache; see [HTTP Cache](/guide/infrastructure/http-cache).

The execution and cache capabilities are independent. A repository can run
actions through E2B or Daytona while storing reusable results through Tuist.
It can also use either capability on its own.

## Available Providers

| Provider | Capability | Environment | Best fit |
| --- | --- | --- | --- |
| [Docker](/guide/infrastructure/docker) | Execution | Local immutable image | Validate per-action graph dispatch in fresh containers. |
| [Microsandbox](/guide/infrastructure/microsandbox) | Execution | Local image | Validate an action boundary on the current computer. |
| [E2B](/guide/infrastructure/e2b) | Execution | Hosted template | Start a prepared hosted environment quickly. |
| [Daytona](/guide/infrastructure/daytona) | Execution | Hosted container image | Build a hosted environment from a familiar image. |
| [Tuist](/guide/infrastructure/tuist) | Shared cache, artifacts | Not applicable | Reuse action results and share installable applications. |
| [Object storage](/guide/infrastructure/object-storage) | Shared cache | Your S3, Google Cloud Storage, or Azure bucket | Share results through storage you already run, without a cache service. |
| [HTTP cache](/guide/infrastructure/http-cache) | Shared cache | Any server with `GET`, `HEAD`, and `PUT` | Reuse a build cache your CI runners already reach. |

## Keep A Local Escape Hatch

The local cache remains available after a shared cache is configured. Set
`STRUO_CACHE_PROVIDER=local` for one invocation:

```sh
STRUO_CACHE_PROVIDER=local ./scripts/greet.sh
```

For execution, omit `--remote` to run on the current computer. Pass
`--compute <provider>` with `--remote` to choose a different named execution
provider for one invocation.

## Next

Start with [Remote Execution](/guide/infrastructure/remote-execution), then open
the setup guide for the provider you want to use.

For CI orchestration, read
[Event-Driven Automation](/guide/infrastructure/triggers). Declare triggers
that select existing capabilities and let the CI service schedule workers
without maintaining another build graph. The guide lists service integration
status, including upcoming Tuist support and a
[Buildkite dynamic-pipeline example](/guide/infrastructure/buildkite).
