---
prev: false
---

# Infrastructure

Once uses a local cache by default. You do not need infrastructure
configuration to run a cacheable script or graph target on one machine.

## Start Locally

Run the [scripted workflow](/guide/scripted/) twice:

```sh
./scripts/greet.sh
./scripts/greet.sh
```

The first run reports a cache miss. The second reports a cache hit and restores
the declared output from the local cache.

This local behavior remains available after a shared provider is configured.
Set `ONCE_CACHE_PROVIDER=local` for one process when you want to bypass the
workspace's remote cache setting:

```sh
ONCE_CACHE_PROVIDER=local ./scripts/greet.sh
```

## Share Results Across Machines

Add a named provider to the repository root `once.toml` when teammates or
automation runners on different machines should reuse the same results. Bind
the cache capability to that provider:

```toml
[infrastructures.tuist]
kind = "tuist"
account = "acme"
project = "app"

[infrastructure.cache]
provider = "tuist"
```

The provider table holds shared connection settings. The capability table
chooses which named provider supplies the cache.

After authenticating, run the same script on two machines with identical
inputs. The first machine reports a cache miss and uploads the result. A second
machine whose local cache has not seen the result should report a cache hit and
restore the output. That hit verifies that the configured provider supplied the
result.

## Configure Remote Execution Separately

A provider can supply more than one capability. Bind remote execution only when
commands should also run on that provider:

```toml
[infrastructure.execution]
provider = "tuist"
project = "preview-execution"
```

The capability can override the provider's default project. With this
configuration, `--remote` uses the bound execution provider:

```sh
once exec --remote -- bash scripts/greet.sh
```

Pass `--compute <provider>` to choose a different execution provider for one
command.

## Available Providers

- [Tuist](/guide/infrastructure/tuist) provides shared cache and remote
  execution.

## Next

Follow the [Tuist setup guide](/guide/infrastructure/tuist) to configure
authentication and verify a shared result. Continue to
[Memory](/guide/memory/) to inspect the evidence that Once records after work
runs.
