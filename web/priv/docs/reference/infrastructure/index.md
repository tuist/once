# Infrastructure Provider Contracts

These references describe the boundaries a service must satisfy to supply
shared caching or remote execution to Once. Start with
[Become an Infrastructure Provider](/guide/infrastructure/providers) for a
top-to-bottom cache integration guide.

## Independent Capabilities

- **[Cache](/reference/infrastructure/cache):** store opaque blobs and action
  results; restore verified content across machines. Current implementations
  are local storage and a local tier backed by Tuist's REAPI cache.
- **[Execution](/reference/infrastructure/execution):** run a prepared command
  in an isolated environment and return declared outputs, streams, and exit
  status. Current implementations are native local execution, Docker,
  Microsandbox, E2B, and Daytona.
- **[Artifacts](/guide/infrastructure/artifacts):** retain immutable build
  products for distribution. Local publication accepts arbitrary payloads;
  see each provider's guide for supported formats and publication limits.

Once owns graph analysis, dependency ordering, action identity, and cache
policy. Providers do not need to interpret target kinds or implement the
graph. [Live run events](/reference/events/) are an optional reporting
protocol, not an execution or cache protocol.

## Integration Status

Provider dispatch is currently compiled into Once. There is no dynamic
provider registration API, generic REAPI cache configuration, or REAPI
`Execution.Execute` transport. A new provider needs an adapter and
configuration/resolution changes in Once. The cache reference separates the
provider-neutral behavior from the wire format used by today's Tuist adapter;
the execution reference describes an internal command boundary and service
requirements, not a new network API.

## Code Reading Map

These links follow `main`. The internal types are implementation references,
not compatibility promises of the public cache SDK. Pin a revision in your
integration's validation record.

- [`InfrastructureProviderConfig` and manifest decoding](https://github.com/tuist/once/blob/main/crates/once-frontend/src/cache_provider.rs):
  supported kinds and the independent named cache/execution bindings.
- [CLI provider resolution](https://github.com/tuist/once/blob/main/crates/once-cli/src/cache_provider.rs):
  resolve workspace policy to a cache implementation or `RemoteExecution`;
  keep credentials separate.
- [Shared cache configuration resolution](https://github.com/tuist/once/blob/main/crates/once-frontend/src/cache_resolution.rs):
  cache selection and overrides used by callers.
- [`CacheProvider`](https://github.com/tuist/once/blob/main/crates/once-cas/src/provider.rs):
  local/remote cache dispatch and the operations consumers rely on.
- [`Executor` selection](https://github.com/tuist/once/blob/main/crates/once-core/src/executor/selection.rs):
  dispatch, resource domain, and unsupported-policy rejection.
- [`PreparedCommand` and remote dispatch](https://github.com/tuist/once/blob/main/crates/once-core/src/remote.rs):
  command data passed into a sandbox adapter.
- [`Provisioner`](https://github.com/tuist/once/blob/main/crates/once-cli/src/provision.rs):
  optional `once connect` support, currently Tuist only.
- [Authentication command dispatch](https://github.com/tuist/once/blob/main/crates/once-cli/src/commands/auth.rs):
  optional interactive/CI login integration, currently Tuist only.

Keep a new provider's transport, authentication, and service behavior in its
adapter. Use generic cache, scheduling, and action primitives for shared
behavior. Do not add ecosystem-specific scheduling or build-system branches
to a provider integration.

See the [manifest reference](/reference/manifest#root-only-tables) for the
configuration customers use and the [provider guide](/guide/infrastructure/providers#handoff-checklist)
for the integration handoff checklist.
