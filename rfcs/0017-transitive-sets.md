# Transitive sets

Status: accepted. Provider sets and dependency-input deltas are implemented.
Composed input digests, lazy declared arguments and compact historical
fingerprints are under integration and measurement.

## Motivation

Target kinds pass transitive data up the graph as flat lists. Every target
copies its dependencies' lists into a new list, deduplicates it, and returns it
in its provider. The cost of one target is proportional to its transitive
closure, so the cost of a graph grows with the sum of all closures: linear for
a flat graph, quadratic for a deep one.

Measured on a generated chain of 800 C libraries, each depending on the one
before it, with four headers per library:

- The provider of the top target holds 3,200 header paths, 1,600 defines and
  800 include directories. Every target holds its own copy of its closure, so
  the session holds roughly 1.3 million path strings in providers alone.
- Providers cross target boundaries as JSON values and are rebuilt in the
  Starlark heap of each dependent, so each copy is made twice more.
- Each record of a target's outcome repeats the same lists as declared inputs,
  input fingerprint components and dependency inputs. The top record is 1 MB,
  and the store reached 391 MiB for 800 targets.
- Struo's own peak memory with saved outcomes disabled was 124, 161 and 286 MiB
  for 200, 400 and 800 targets.

Bazel avoids this with depsets and Buck2 with transitive sets: a set is a node
holding its direct items and references to child sets, built in constant time
per target and flattened only where a consumer needs the items. Struo needs the
same primitive. Per the repository rules it belongs on the Rust side, is
ecosystem neutral, and target kinds compose it.

## Goals

- Building a set costs the target's direct items plus one reference per child,
  independent of the closure size.
- Providers, analysis memos and target outcome records store a set once and
  refer to it by digest everywhere else.
- Action digests do not change in the first step, so adopting sets invalidates
  no caches.
- Flattening is explicit and happens where items are consumed: executing an
  action, materializing its inputs, or an explicit `to_list()` in Starlark.

## Non-goals

- Sets of arbitrary values. Items are strings (paths, flags, labels), which is
  every quadratic list in the prelude today. Structured records such as
  dynamic library descriptors stay lists until a need is shown.
- Sets that keep duplicates. Link options retain duplicates within each
  library's ordered group; the set deduplicates whole groups, not options.
- Projections or per-item callbacks evaluated lazily, as in Buck2. Formatting
  covers the prelude's needs (see Command lines).

## Semantics

A set is an immutable node: an ordered list of direct items and an ordered list
of child sets. Its flattened form is a pre-order walk, direct items first, then
each child left to right, keeping the first occurrence of each item and
skipping any node already visited.

This is exactly what the prelude computes today with
`_unique(own + dep[key] for each dep)`. Because a dependency's list is itself
the flattened form of its set, the two agree item for item, which is what lets
existing actions keep their digests. Skipping visited nodes is sound because a
visited node's items were all emitted already, and it keeps diamond-shaped
graphs linear.

Preorder remains the default. Explicit topological flattening uses reverse
postorder with right-to-left child traversal, placing shared dependencies after
all their dependents while keeping direct items in order. Link-option groups
need this order: first-seen preorder deduplication can place a static archive
before another archive that needs its symbols. Traversal order does not change
the set's digest or the default traversal of declared inputs and arguments.

### Identity

Each node has a content digest over a domain-separated encoding of its direct
items and its children's digests. Two targets that build the same set get the
same digest, so the store holds it once. The digest is computed when the node
is created, in time proportional to the node's own size.

## Starlark surface

```python
headers = transitive_set(
    direct = [src] + own_headers,
    transitive = [dep.get("transitive_headers") for dep in ctx["deps"]],
)
```

- `transitive_set(direct = [], transitive = [])` returns a set. Entries of
  `transitive` may be sets, `None` (skipped), or lists. A list is wrapped as a
  leaf node, so a kind can adopt sets before the kinds it depends on do.
- `set.to_list()` flattens. Iteration, `len`, `in` and `+` are rejected with a
  diagnostic that names `to_list()`, so an accidental flatten is visible
  rather than silently quadratic.
- `set.to_list(order = "topological")` flattens with parents before children,
  including shared children. The default is `order = "preorder"`; other names
  are rejected.
- `run_action(inputs = ...)` accepts a mix of strings and sets.
- `cmd_args(args = ..., format = "-D{}", before_each = "-I")` accepts sets in
  `args`. `format` rewrites each item and `before_each` inserts a separate
  argument before each item. `argv` already accepts `cmd_args` values.
- `value_to_json` and `json_to_value` carry a set across target boundaries as
  `{"$struo_transitive_set": "<digest>"}`. The analysis session interns nodes
  by digest, so a dependent receives the same node, not a copy.

The common prelude helpers (`_collect_transitive` and the per-family copies of
it) switch to returning sets. Call sites that do list operations on the result
call `to_list()` instead; the rejection diagnostics find every one of them.

The public Starlark modules reference gains a Transitive Sets section, and the
Actions section documents sets in `inputs` and `cmd_args`, in the same change
as the globals.

## Rust shape

- A `TransitiveSet` node type in the frontend: `Arc` of direct items as shared
  strings, child `Arc`s and the digest. It is a simple Starlark value (no heap
  references), so it can be shared across target heaps and threads without
  freezing or copying. Registering a Starlark value type expands to `unsafe
  impl`s from starlark's derives, so the value type sits in one module with a
  scoped `unsafe_code` exemption and no hand-written unsafe code.
- A session-scoped interner from digest to node, so equal sets built by
  different targets share memory and JSON references resolve without copying.
- A flatten iterator that streams items with a visited set of node digests and
  an item set, so callers never need an intermediate list.

## Storage

Records reference sets by digest. Nodes are written content-addressed beside
the records that use them, each holding direct items and child digests, so the
whole graph's sets take space proportional to the sum of direct items rather
than the sum of closures. Target outcomes and analysis memos reference the
same nodes. Historical input fingerprints use a separate content-addressed
component DAG because their leaves include resolved content digests and
categories, rather than only strings.

Fingerprint nodes live in a packed SQLite store. Content-derived boundaries
group components into blocks, capped at 256 components and one mebibyte each.
A Cartesian tree references separate block blobs, so changing a tree ancestor
does not copy its component payload. A retained writer reuses verified nodes
through a bounded cache and publishes each fingerprint in one transaction.
Readers authenticate branches and blocks and expand evidence only when
loading it. Evidence queries select subjects and apply their history limit in SQLite
before expanding fingerprints, so requesting a few records does not allocate
the whole history. Legacy inline evidence remains readable. Outcome cleanup
must not remove nodes that historical evidence still references. Node garbage
collection is not implemented yet.

A record or memo that refers to a missing node is treated as missing, the same
way a missing record is today: that target is visited again.

## Action identity

Declared actions retain input and argument set references through analysis
and persistence. The input digest composes each node from its direct resolved
inputs and child digests. Leaves bind category, path and content digest:
the same path set can resolve to different generated contents in different
actions. Argument metadata binds the compact ordered fragments, including
formatting and repeated arguments. Replacing a flat list with a structurally
different set can therefore change the key even when execution sees the same
strings.

The input digest domain advances to v6, and the default action digest domain
advances to v12. Run-command snapshot and exit-policy domains advance too.
Old cache entries do not alias the new encoding.

The core action still contains flat execution inputs and arguments. Its digest
writer streams their JSON into the hash through a bounded buffer. Shared input
nodes are memoized within one action, not across actions, and public evidence
manifests still expand to flat components. These paths remain proportional to
the sum of closures. Graph-linear hashing needs context-safe cross-action
memoization and a composed core execution identity; this change does not
claim that property.

## Steps

Each step lands on its own and is measured on the chain and flat fixtures plus
one real workspace before the next begins.

1. **Sets in providers.** Add the node type, interner, `transitive_set`,
   `to_list` and the JSON reference. `run_action` and `cmd_args` accept sets
   and flatten them when the action is declared. Migrate the C family first,
   then Rust and Apple, which hold most of the prelude's transitive lists.
   Action digests are unchanged; a test compares the digests of every bundled
   example before and after migration. Expected effect: provider memory and the
   provider field of outcome records drop from the sum of closures to the sum
   of direct items.
2. **Per-target records.** Profiling after step one showed that the closure
   copies left in declared actions and outcome records were not mostly
   strings a set can hold. On the 800-library chain the top record was
   833 KB: 469 KB of input fingerprint components, 280 KB of dependency
   inputs (every output beneath the target with its digest) and 93 KB of
   `sources`. Struo's peak heap was 131 MiB, 102 MiB of it lowered actions
   the recovery registry kept for re-running a producer whose outputs went
   missing.
   - The recovery registry keeps each action as compressed JSON and checks
     its digest when it reads one back, which only a re-run does. Peak heap
     on the chain fell to 89 MiB, and after startup the heap stays under
     23 MiB.
   - A record stores how the target changed the inputs its dependencies made
     available: its own outputs and the paths they replaced. The record key
     already hashes the dependency inputs' paths and digests, so a record
     whose key matches is rebuilt from the same base. The top record holds
     2 entries instead of 1,600, the store fell from 322 MB to 217 MB, and
     peak RSS for a one-file edit fell from about 130 to 120 MiB.
   - Declared action `inputs` and argv stay flat lists. Lowering sorts and
     deduplicates inputs before hashing, so a set would be flattened there
     anyway, and `sources` is a ninth of a record. `sources` cannot simply be
     reduced to the target's own files either: a dependency whose actions
     never read its own headers would not notice an edit to them.
   - Dependency inputs are still merged into one map per target in memory.
     The merge is transient and the heap after startup is small, so sharing
     it through a persistent map waits for a measurement that needs it.
3. **Composed input digests.** Retain set references in declared inputs,
   command lines, response files and outcome sources. Compose resolved input
   digests from those nodes and bump the action digest domain. Store ordered
   fingerprint components in a shared content-addressed DAG, preserving order
   and duplicates, and expand them on read for the existing evidence API.
   This removes repeated closures from records and shrinks the evidence
   database, which reached 337 MiB on the 800-target chain. Measure the packed
   node store as well as record sizes: one file per component node initially
   regressed cold and incremental time despite smaller records. Cross-action
   hashing and flat core execution records remain separate costs to address.

## Validation

- Unit tests: flattening matches `_unique` concatenation on chains, diamonds,
  repeated children, empty nodes and lists mixed with sets; digests are equal
  for equal content and differ when only child order differs.
- Prelude tests: every bundled example loads without diagnostics and produces
  the same action digests as before migration.
- Memory regression: a test that builds a deep generated chain and asserts the
  summed size of providers grows linearly in its depth, so a kind that
  reintroduces flat transitive lists fails.
- Benchmark: Struo's own peak memory for cold, incremental and no-op builds on
  the chain and flat fixtures, added to the benchmarks directory.

### Current integration measurements

On the 800-library chain with a populated action cache, the block store reduces
target-outcome storage from 217 MiB to 6.4 MiB. A five-record evidence query
reduces peak RSS from about 556 MiB to 24 MiB by selecting rows before
expansion. The public records retain the same subjects and component labels.

Before the startup fix, a clean-metadata build takes 8.58 seconds
instead of 6.51 seconds, with peak RSS of 166 MiB instead of 154 MiB. A
one-file edit takes 1.51 seconds instead of 1.21 seconds, with peak RSS of
123 MiB instead of 120 MiB. These are single release observations in one Linux
environment, not a comparison with another build system. At that point,
compact storage improves, but build-process memory does not.

A subsequent startup fix evaluates built-in Starlark sources one at a time
during schema discovery, while keeping workspace module code together. In an
isolated release comparison against the block-store candidate, discovery peak
live heap falls from 86.54 MB to 61.00 MB. Median query RSS across five fresh
processes falls from about 101 MiB to 77 MiB; query time increases from about
98 ms to 106 ms. The 800-library clean-metadata build falls from 161 MiB to
139 MiB, with elapsed times of 8.44 and 8.17 seconds. An edit falls from
120 MiB to 97 MiB, with elapsed times of 1.46 and 1.33 seconds. Build readings
are single observations, and the broader build-time gate remains open.

After profiling build time, input composition borrows path keys and resolves
each path through one map entry lookup. Generated-input lookup skips ancestor
searches when the ordered output range excludes a match. Fingerprint encoding
reuses a serialization buffer, and digest JSON uses stack-backed hex encoding
and single-pass decoding.

Fingerprint reads reuse parsed blocks within a retained store, bounded to
2 MiB of accounted payload allocations and 256 index entries. SQLite still
reads and authenticates each blob before cache lookup, so edits or deletions
after an earlier successful decode still fail. The cache avoids JSON parsing,
not authentication or public manifest expansion. These bounds do not reserve
memory through ResourcePool.

Three alternating release trials on matching C800 fixtures give these medians:

| Measure | Before step 3 | Before build-time fixes | After build-time fixes |
| --- | --- | --- | --- |
| Clean metadata, seconds | 6.34 | 8.54 | 6.48 |
| One-file edit, seconds | 1.06 | 1.35 | 1.10 |
| No-op, seconds | 0.02 | 0.02 | 0.02 |
| Clean metadata, sampled Struo RSS, MiB | 154 | 142 | 144 |
| One-file edit, sampled Struo RSS, MiB | 120 | 101 | 106 |

These runs reuse a populated action cache and exclude compiler subprocesses
from sampled RSS. Host timing varies: the original clean runs span 6.29 to
7.39 seconds. The final median remains about 2% above the original for clean
metadata and 4% for edits, so these trials show recovery near the baseline,
not proof of performance parity across workloads. All 800 final target records
retain the pre-fix action/input digests, compact fingerprint references, source
references, providers and outputs.

## Risks and open questions

- Third-party target kinds that read a dependency's transitive field as a list
  break when that dependency migrates. The list-in-`transitive` fallback lets
  readers migrate first; writers in the bundled prelude migrate in the same
  change as their readers. Is a deprecation window needed for project modules
  outside the repository?
- `to_list()` in a hot path reintroduces quadratic cost silently. Analysis could
  record how many items each target flattened and surface large counts in
  `struo query` output.
- Link options keep duplicates and order, so they are not a set of options.
  Concatenating each dependency's list repeated a library's options once per
  path, which on a graph where each library depends on the previous two grew
  with the Fibonacci numbers: 28 libraries pushed Struo's peak memory to
  505 MiB and a build of 60 was killed on an 8 GB host. The C and Zig families
  now keep a transitive set of per-library groups, so each library's options
  appear once and whole. Groups flatten in topological order so shared
  dependencies follow all their dependents. Earlier measurements of the same
  graphs peak at 114 and 118 MiB. Swift on Android still
  concatenates, with dependencies first.
- Item interning across sets (sharing equal strings) is left out; node sharing
  already removes the duplication that matters, and string interning can be
  added inside the node type without changing the surface.
