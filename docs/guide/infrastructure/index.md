# Infrastructure

Infrastructure providers connect Once to shared services for cache storage and
remote execution. The repository root `once.toml` owns this configuration so
every script, graph target, and command-line invocation resolves the same
defaults.

## Providers

Name each provider once under `infrastructures`, then bind capabilities to that
name:

```text
[infrastructure.cache]
provider = "tuist"

[infrastructure.execution]
provider = "tuist"
project = "preview-execution"

[infrastructures.tuist]
kind = "tuist"
account = "acme"
project = "app"
```

In this example, cache and execution both use the `tuist` provider and the same
account. Cache uses the provider default project, while execution overrides the
project with `preview-execution`.

Use this shape when a provider supports more than one capability. It keeps the
shared server, account, and authentication setup in one place, while each
capability can still override the fields that should differ.

## Capability Overrides

Capability tables accept the provider name plus provider-specific fields:

```text
[infrastructure.cache]
provider = "tuist"
project = "app-cache"

[infrastructure.execution]
provider = "tuist"
project = "linux-execution"
```

Pass `--compute <provider>` with `--remote` when a single command should bypass
the configured execution provider. Set `ONCE_CACHE_PROVIDER=local` for a process
that should keep the repository configuration but use only the local cache.

## Available Providers

- [Tuist](/guide/infrastructure/tuist): shared cache storage and execution
  sessions backed by Tuist.
