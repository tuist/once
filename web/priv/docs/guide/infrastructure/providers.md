# Become an Infrastructure Provider

This guide walks through integrating a shared-cache service with Struo, from
defining the service contract to verifying reuse from a fresh local cache.
Use the [cache provider contract](/reference/infrastructure/cache) for the
exact operations, formats, and acceptance checks.

**Integration requirement:** a new provider currently needs an adapter in
Struo. Adding a service URL or a new `kind` to `struo.toml` does not register a
provider. Plan for changes in Struo alongside your service implementation.

## Define the Cache Service

Your service provides two things:

- **Content storage:** read, write, and check the availability of immutable
  blobs addressed by digest.
- **Action results:** look up and publish the result associated with an
  action key, including its exit code and references to output and log blobs.

Struo computes action keys, maintains the local cache, and captures and restores
outputs. The service treats action keys and stored output data as opaque: it
stores and returns bytes rather than interpreting artifacts or deciding when a
build can reuse them. See [content and action results](/reference/infrastructure/cache#content-and-action-results).

## Agree on Identity, Access, and Limits

Before implementing the integration, decide:

1. **Tenant scope.** Which authenticated account/project can read cached
   content and publish results? Authorize every request; a caller-supplied
   account or project name must never grant access by itself.
2. **Credentials.** How do developers and CI authenticate? Keep provider
   credentials out of committed configuration, cache keys, and logs.
3. **Capacity and retention.** Define storage quotas, maximum object sizes,
   request and transfer limits, and eviction policies. Enforce them on your
   service; local cache cleanup does not delete remote data.
4. **Failure behavior.** Distinguish a confirmed cache miss from an outage,
   refused credentials, malformed responses, or corrupt content. An entry
   whose referenced content has been evicted is a miss, not a partial hit.

## Implement the Adapter and Service Together

Implement the [provider-neutral cache operations](/reference/infrastructure/cache#provider-neutral-operations)
and register your provider kind with Struo's manifest parsing and provider
resolution. The [provider reference](/reference/infrastructure/) maps those
parts of the code.

Preserve these behaviors in the adapter:

- Keep the local cache and consult it before fetching remote content.
- Publish referenced blobs before publishing the action result.
- Verify downloaded bytes before making them available locally; use bounded
  streaming and the shared transfer budget for large objects.
- Preserve the distinction between missing data and failures so Struo can
  apply its cache policy.

The existing Tuist adapter uses a Bazel-compatible cache protocol (REAPI).
If you reuse this transport, preserve its digest mappings and Struo result
metadata, along with authentication, endpoint discovery, and tenant scoping. See the
[wire contract](/reference/infrastructure/cache#existing-reapi-transport).
Other transports must preserve the same provider-neutral behavior.

Document how users configure a named provider in `[infrastructures.<name>]`
and select it through `[infrastructure.cache]`. Use the existing manifest
structure rather than introducing another configuration file.

## Make Onboarding Optional and Explicit

Start with a manually provisioned account and project, documented credentials,
and a working cache binding. Automated project creation is not required for
the cache to work.

If you also support `struo connect`, implement session detection, project
listing and creation, and useful permission and conflict errors. The current
provisioner and login dispatch support Tuist only; another provider needs its
own integration, not a copy of Tuist's HTTP routes. See
[Connect a Project](/guide/infrastructure/connect).

## Validate a Cache Without a Local False Positive

Check that another client can reuse a result stored on your service:

1. In a disposable project, publish an action result and its referenced blobs.
2. Use a second client with an empty local cache, configured for the same
   service, account, and project, to retrieve that result and its blobs.
3. Confirm that the result matches and the restored bytes are identical.

The empty local cache matters: otherwise a successful read could come entirely
from the first client's local storage. A successful write alone is not proof
of remote publication, because the current adapter treats remote writes as
best-effort.

Before shipping, also verify eviction, denied access, outages, corrupt content,
interrupted transfers, and isolation between tenants. The
[cache acceptance checks](/reference/infrastructure/cache#acceptance-checks)
describe the expected outcomes. Use disposable data for these tests.

## Handoff Checklist

Before customers use the integration, publish:

- The supported Struo revision, provider kind, `struo.toml` example, and
  developer/CI authentication instructions.
- Storage and transfer limits, retention and eviction policies, and the
  permissions needed to read and publish entries.
- Evidence of reuse from an empty local cache, byte-for-byte restoration,
  tenant isolation, and the failure checks above.
- Operational ownership and incident procedures for the cache service.

Continue with the [cache provider contract](/reference/infrastructure/cache)
for implementation details and the [provider reference](/reference/infrastructure/)
for the code map.
