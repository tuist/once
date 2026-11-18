---
prev: false
next: false
---

# Software Development Kit

Use a Struo language library when an application needs to store and retrieve
cache data directly. Use the [command line](/reference/cli/) instead when the
goal is to execute scripts, build targets, run tests, or manage runtime
sessions.

The language libraries intentionally expose the same small cache surface:

- Open the default local cache, an isolated local root, or the effective
  provider configured for a workspace.
- Store and retrieve content-addressed byte payloads.
- Store and materialize files without loading their complete contents into the
  host language.
- Build versioned action keys from labeled values and content digests.
- Associate an action digest with a completed action result.
- Remove an action result without deleting its referenced payloads.
- Inspect local cache statistics.

They do not execute commands or load the repository graph.

## Choose A Language

| Language | Package | Call style | Guide |
| --- | --- | --- | --- |
| Rust | `struo` crate | Asynchronous | [Rust](/guide/sdk/rust) |
| Swift | `Struo.xcframework` and the matching Swift wrapper | Asynchronous | [Swift](/guide/sdk/swift) |
| Ruby | `buildstruo` gem | Synchronous | [Ruby](/guide/sdk/ruby) |
| JavaScript | `buildstruo` package for Node.js | Promise-based | [JavaScript](/guide/sdk/javascript) |
| C | `libstruo` and `struo.h` | Synchronous | [C](/guide/sdk/c) |

Choose the binding used by the process that owns the integration. All
bindings use compatible digests and action-result concepts, so separate tools
can share a configured cache without translating records.

## Choose A Cache

The default constructor preserves the lightweight local behavior of earlier
releases. Use an explicit local root for test isolation or caller-owned cache
lifetimes. Use a workspace constructor when the integration must share the
same effective provider as the command line. Workspace selection considers
the process override, workspace infrastructure, compatible Tuist project
configuration, and the user default in that order.

Statistics describe the local tier. Remote uploads are best effort: a
configured remote provider writes through the local tier and keeps the local
write usable when the remote service is unavailable. Streamed inputs larger
than eight mebibytes remain in the local tier to preserve bounded memory use.

## Misses, Failures, And Integrity

Reads distinguish a miss from a failure. A missing action result, or a blob
existence check that answers false, means the cache confirmed the content is
absent. That includes a remote action result whose referenced blobs the remote
service confirms are gone, which is reported as a miss so the action can run
again and republish it.

A remote service that cannot be reached, times out, rejects the credentials,
or returns a malformed entry produces an error instead. Earlier releases
reported some of these failures as a miss or as a missing blob. Integrations
that want to keep working through a remote outage should catch the error and
decide whether to run the work without reusing a cached result, rather than
treating the outage as proof that nothing is cached. Uploads remain best
effort.

Idempotent remote reads and content-addressed writes are retried a bounded
number of times with randomized backoff and an overall time limit. When the
remote service says how long to wait before retrying, the client waits at
least that long, within the same time limit; a quota error is retried only
when the service gives such a hint. Refused credentials and malformed
requests are never retried.

A failed connection, endpoint lookup, or credential service outage is retried
after a cooldown instead of disabling the remote tier for the rest of the
process. One attempt runs at a time and every waiting caller shares its
result, even when the caller that started it is cancelled. Rejected or
missing credentials are not retried and ask you to sign in again.

Blobs are verified against their digest before they are used. A download is
checked before it is committed to the local tier, and a response larger than
its digest declares is rejected. Only small blobs are transferred from the
remote service through memory; larger ones stream to disk in bounded chunks,
so the size a remote entry claims never decides how much memory a transfer
uses. Reading a whole blob into memory remains the caller's choice; copy large
blobs to a file instead. Restoring a blob hashes it while it is written, so
the destination only receives verified content. A local blob that fails the
check is removed and reported as corrupt; with a remote provider it is
fetched again once before the read fails.

## Transfer Budgets

A remote provider bounds the memory buffers and scratch disk its transfers
use. A streamed download reserves scratch space for both its declared raw
size and its bounded encoded representation before it starts. This is slightly
more than twice the declared size, so a staging budget that previously fitted
only one raw copy can now refuse the transfer. Encoded bodies cannot grow past
that allowance. Transfers and cache lookups also reserve their bounded buffers.
Cancellation releases capacity only when the physical worker has stopped and
its temporary storage is no longer needed. Providers cloned from one another share
their budget; separately constructed providers each have their own, and the
command line shares one budget across the providers it resolves.

By default a provider allows 256 mebibytes of transfer buffers and 32
gibibytes of staged downloads. Budgets are counted in 4 kibibyte units:
configured sizes round down and requests round up. Work whose buffers or
download do not fit the whole budget fails with an exhausted error before
anything is sent, rather than running with buffers larger than the budget
allows; earlier releases had no such limit.

Every remote operation runs within a memory budget of at least 32 mebibytes
and 64 kibibytes, which the Rust crate exposes as its minimum transfer memory.
Streamed downloads reserve at least that minimum, including bounded codec
working space and producer queues. Their response-message ceiling is reduced
when necessary to leave room for those buffers; it never exceeds 32 mebibytes.
Cache lookups, small blob reads, and
uploads streamed from a file need less. Uploading a buffer the caller already
holds reserves about twice its size, so buffers larger than roughly 15
mebibytes need more than the minimum. With a smaller budget, the operations
that need more fail as exhausted; they are never treated as a cache miss.

These budgets are an accounting of the buffers the cache sizes for its
transfers, not a limit on process memory. Connecting, endpoint discovery,
authentication, capability discovery, transport internals, and upload
compression working memory are outside them.

Error messages and logs name remote endpoints by scheme, host, and port only,
and describe remote failures by category and status code. Text a server sends
back, such as a status message or response body, is not repeated because it
can contain credentials.

## Before You Embed Struo

Use the action-key builder to partition each integration by namespace and add
every input that can affect the result. Input order is significant and must be
deterministic. The library cannot determine whether an application omitted an
input or whether replaying a result is safe.

Start with the smallest useful operation in the language guide, then add
action-result storage only after the application has a deterministic action
identity.
