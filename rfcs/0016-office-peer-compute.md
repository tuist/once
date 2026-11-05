# Request for Comments 0016: Office Peer Compute

## Status

Draft. The peer blob mirror waits for the measurement described below. Peer
execution waits for the prerequisites listed in its section.

## Summary

Teams that work in the same office have idle machines that often share most of
a build's content. Once could use them in two steps:

1. **Peer blob mirror.** Fetch content-addressed blobs from colleagues on the
   local network before the remote cache. Action results still come only from
   the trusted cache provider, so a peer can never decide what an action
   produced.
2. **Peer execution.** Later, run eligible actions on a colleague's machine
   through the executor and resource-domain model from
   [RFC 0014](0014-executors-and-resource-domains.md).

The first step needs no remote code execution and no new trust in a peer's
results. The second step depends on executor gates that are not met yet.

## Motivation

[Remote execution](0004-remote-execution.md) assumes capacity that someone
provisions and pays for. An office already has that capacity: many developer
machines, mostly idle during meetings, reviews and reading. Those machines
also build the same repository, so they hold much of the content a colleague
is about to download.

Earlier tools show both the value and the risks. Xcode distributed builds used
Bonjour to find peers and were later removed. distcc, icecream, Incredibuild
and sccache-dist distribute compilation with varying success. Common failure
reasons include laptop cores improving faster than office networks, compilers
whose work does not split into independent units, toolchain mismatches that
produce wrong but plausible outputs, and owners disabling the feature because
it slowed their own machine.

Once avoids part of this because it owns the action graph and a
content-addressed store (CAS). It does not avoid toolchain identity, trust or
owner experience. This RFC addresses those directly.

## Non-Goals

- **Consensus.** Peers do not agree on shared state. Each blob is verified by
  its digest, and each future execution lease is a contract between one
  client and one peer. Raft, Paxos or gossip-based membership agreement would
  add failure modes without a matching problem.
- **A central office scheduler.** Each client places its own work. A
  coordinator only becomes useful if fairness across many simultaneous
  clients is a measured problem.
- **Replacing the remote cache.** Peers are an accelerating tier. A peer
  outage or a lying peer must leave the build as correct and as fast as it is
  without peers.
- **Implicit background daemons.** A machine serves peers only when its owner
  opts in.

## Step 1: Peer Blob Mirror

### Lookup Order

Action results keep their current lookup path: local store, then the
configured cache provider. Peers never serve action results.

Blob lookups become local store, then eligible peers, then the configured
cache provider:

```text
action result lookup:  local -> cache provider
blob lookup:           local -> peers -> cache provider
```

A client asks peers only for digests it learned from a trusted action result
or from its own declared inputs. A peer response is either a stream of bytes
or a refusal. The client streams the bytes into a staged local blob, hashes
them while writing, and publishes the blob only if the digest matches. This
reuses the existing staged, verified store path. A mismatch counts as a
corrupt peer response: the staged data is discarded, the peer is penalized
for the rest of the session, and the lookup continues with the cache
provider.

### Failure Semantics

The peer tier must preserve the existing cache error categories:

- A peer miss is never evidence of absence. Only the authoritative cache
  provider can report that a blob is not found.
- A peer timeout, refusal, disconnect or corrupt response falls through to the
  cache provider. It never becomes a missing input, a cache miss, or a build
  failure by itself.
- A peer that does not start streaming within a short deadline is abandoned in
  favour of the cache provider. Hedged requests count against the same
  transfer budget as every other download, and the losing transfer is
  cancelled and accounted for until it stops.
- Single-flight download ownership stays as it is: one transfer per digest per
  client, whether it comes from a peer or the cache provider.

### Discovery

Peers announce themselves with DNS service discovery over multicast DNS on the
local link, for example as `_once-cas._tcp`. Announcements carry only a
protocol version, a port and a keyed namespace fingerprint. They do not carry
account names, project names, user names or digests. A client considers only
peers whose fingerprint matches its own cache namespace.

Networks that block multicast can list peers explicitly in `once.toml`.
Discovery results are cached briefly, and peers that fail repeatedly are
removed from the candidate list for the rest of the session.

### Finding Content On Peers

Discovery produces a list of candidate peers, not a routed network. A client
talks to each peer it chooses directly. Peers do not forward requests, relay
content or maintain a shared view of the office.

Asking every peer about every blob does not scale: a warm build can need
thousands of blobs, and a per-blob broadcast would load every participating
machine. The client instead uses two mechanisms, added in this order:

1. **Batched presence queries.** Before downloading, the client sends a small
   number of peers one request listing many digests, and each peer answers
   with the subset it holds. This mirrors the missing-blob exchange in the
   Remote Execution API. Batch size, the number of peers asked and the query
   rate are bounded. Presence answers are hints: a peer may evict a blob
   between the answer and the download, and that falls through like any other
   peer miss.
2. **Availability summaries.** If measurements show presence queries
   dominating peer traffic, each serving peer also publishes a compact
   probabilistic summary of its store, such as a Bloom filter, refreshed
   periodically. Clients test digests against summaries locally and query only
   peers that probably hold them. A false positive costs one refused request; a
   stale summary costs the same.

A summary reveals what a machine has built, so it is never part of the
multicast announcement. Peers exchange summaries only after mutual
authentication, and summaries are keyed by the cache namespace so that one
team's summary cannot be tested against digests computed elsewhere.

Peers serve only what their own builds placed in their local stores. They do
not push, prefetch or replicate content to each other, and there is no
assignment of digests to peers. A distributed hash table was considered and
rejected: laptops join and leave throughout the day, and a peer's contents
follow its owner's work rather than any placement rule.

### Authorization

Knowing a digest is not authorization to read the content. Blobs contain the
team's source code and build outputs, so every request must prove membership
in the same cache namespace that the remote provider already enforces.

The preferred design asks the cache provider to issue short-lived, signed peer
credentials scoped to one account and project. A serving peer verifies the
signature, scope and expiry offline, so the office network does not need to
reach the provider for every request. Both sides authenticate over mutual TLS
with keys bound to those credentials. Revoking a member at the provider stops
new credentials, and expiry bounds the remaining window.

A shared team secret configured through `once auth` is simpler, but it cannot
be revoked per person. It is an acceptable fallback for evaluation, not the
default.

### Serving

Serving is an explicit command, such as `once cache serve --peers`, that the
owner runs or installs as a login item. It:

- serves blobs from the local store only, read-only, and never writes on
  behalf of a peer;
- admits each upload through its own resource budget, with bounded
  connections, bounded buffers and a configurable bandwidth limit;
- streams files instead of loading them into memory;
- pauses or refuses requests when the machine is on battery or under thermal
  pressure, if the platform reports those states;
- logs per-request outcome, bytes and duration through `tracing`, without
  digests of unrelated content or peer credentials.

### Configuration

Peers attach to the existing cache configuration. They do not introduce a new
manifest or a second configuration layer:

```toml
[infrastructure.cache]
provider = "tuist"
peers = "office"

[infrastructures.office]
kind = "lan_peers"
# Optional, for networks that block multicast DNS.
addresses = ["10.0.4.12:7643", "10.0.4.31:7643"]
```

Workspace configuration can enable peer lookups for everyone. Serving remains
a per-machine choice made through the command above.

### Measurement Before Building

The mirror only helps if remote cache downloads are a meaningful share of
build time. Before implementing it, measure on a real office network:

- the share of wall time that warm builds spend waiting on cache downloads,
  using the transfer records already written to session logs;
- how often a blob one developer downloads was already downloaded by a
  colleague in the previous few hours, estimated from provider access logs;
- office uplink bandwidth and latency to the cache provider compared with
  local network bandwidth between developer machines.

If downloads are a small share of warm builds, or colleagues rarely share
blobs within a working day, the mirror should not be built. A single on-site
cache mirror is also a simpler alternative to compare against.

## Step 2: Peer Execution

### Model

A participating machine is an executor bound to its own resource domain, as
RFC 0014 already anticipates for directly managed workers. Its owner decides
how much capacity it lends, and that capacity is a domain the peer enforces,
not a promise the client assumes.

```text
action + execution requirements
  -> eligible peers (exact identity match)
  -> lease request to one peer
  -> peer admits against its lent domain
  -> prepared command in a fresh sandbox
  -> verified outputs returned to the client
```

### Leases

A client requests an exclusive lease for one action. The peer accepts or
refuses according to its own load and owner policy. Leases expire on their
own, so a client that sleeps or leaves the network does not hold capacity
indefinitely. A peer that disappears mid-action leaves the action in an
uncertain state; only duplicate-safe actions may run again elsewhere.

### Eligibility

Placement filters before it ranks, as RFC 0004 requires. A peer is eligible
only when it matches the action's execution platform exactly: operating
system, architecture, immutable tool and SDK identities, isolation and
network requirements. For native macOS work this includes the Xcode build
number, SDK versions and macOS version, ideally as a toolchain digest. A
near match is not eligible. Among eligible peers, the client ranks by
estimated finish time, including missing-input transfer, which the blob
mirror makes small.

### Trust In Results

Running a colleague's action executes their code, and accepting a peer's
outputs trusts that peer. Peer execution therefore requires:

- mutual authentication with the same credentials as the blob mirror;
- sandboxed execution in a fresh workspace, temporary directory and home;
- results that stay in the requesting client's local store and are not
  published to the shared cache provider, unless the team explicitly trusts
  peer provenance;
- no execution of actions that need signing credentials or other secrets.

### Owner Experience

Lending compute is opt-in and conservative by default. A peer backs off when
its owner is actively using the machine, when it is on battery, under thermal
pressure, or in a screen-sharing session if the platform reports it. Lent
capacity is visible to the owner and can be paused at any time.

### Prerequisites

Peer execution should not start until these gates from the
[executor failure-mode analysis](0014-executor-failure-modes.md) are met:

- **Cross-executor sharing:** immutable tool and environment identities,
  including native macOS toolchains, normalized execution paths and cache
  trust partitioning.
- **Remote graph execution:** executor eligibility, domain-aware graph fan-out,
  producer provenance and bounded lost-input recovery.
- **Shared workers:** subscriber-independent ownership with completion and
  cancellation race tests.
- **Fallback or racing:** classified failures, independently eligible branches,
  isolated outputs, one publication owner and loser lifecycle accounting.

It also needs target kinds to emit actions that are large and hermetic enough
to justify a network round trip. Linking and signing stay local. Swift
compilation needs batch-level frontend actions rather than one opaque build
invocation.

## Alternatives Considered

- **A central office scheduler**, as in icecream. It improves fairness across
  clients but adds a single point of failure and an operational service. It
  can be added later if measurements show contention between clients.
- **An on-site cache mirror or executor pool.** One dedicated machine running
  a cache mirror or remote execution service is simpler to secure and to
  reason about. It requires hardware and someone to maintain it. Peer
  discovery uses capacity that already exists. Measurements should compare
  both.
- **Swarm-style blob distribution.** Splitting blobs into chunks fetched from
  many peers at once helps very large artifacts, but adds protocol complexity
  before there is evidence that single-peer transfers are the bottleneck.

## Validation Requirements

The blob mirror needs integration tests that show:

- a peer returning wrong bytes leaves no blob in the local store, and the
  build succeeds from the cache provider;
- a peer timeout, refusal or disconnect never surfaces as a missing blob;
- a request without valid credentials, with an expired credential or for
  another namespace is refused;
- announcements contain no account, project, user or digest information;
- serving respects connection, buffer and bandwidth limits under many
  concurrent requests;
- a hedged request cancels and accounts for the losing transfer;
- presence queries stay within their batch, fan-out and rate bounds during a
  large warm build;
- a blob a peer reported as present but evicted before download falls through
  to the cache provider;
- availability summaries are refused to unauthenticated clients and never
  appear in announcements.

Peer execution needs its own validation plan once its prerequisites are met.

## Open Questions

- Can the cache provider issue offline-verifiable, project-scoped peer
  credentials, and how short should they live?
- What deadline should a peer get before the client hedges to the cache
  provider, and should it adapt to observed latency?
- How many peers should a client query per batch, and how should it choose
  them before any summaries or history exist?
- What summary size and refresh interval keep false positives and staleness
  acceptable for a typical local store?
- Is a single on-site mirror good enough for most teams, making peer
  discovery an optimization for offices without one?
