# Fabrik: Execution Roadmap

A phased plan optimized for **shortest path to self-hosting**. The design spec ([design.md](design.md)) describes the destination; this document sequences how we get there.

## Sequencing principles

1. **Self-hosting is the forcing function.** Every phase is judged by how much closer it gets us to "Fabrik builds Fabrik faster than cargo." Architectural elegance that doesn't move that needle waits.
2. **Defer architectural complexity until the system works.** WASM plugins, the DSL, REAPI, and the query API are all in the v1 design — but each blocks self-hosting if we build them first. We bring them in once we have a working concrete system to extract them from.
3. **Every phase ships a working artifact.** No phase is "refactor with no user-visible change." If a phase doesn't produce something runnable that's better than the prior phase, the phase is wrong.
4. **In-process before out-of-process.** Plugins start as Rust modules linked into the binary. The plugin *contract* (typed interface) gets designed early; the plugin *isolation* (WASM) comes later. This lets us iterate on the contract against real code.
5. **Hardcode before generalize.** The Rust language support is hardcoded at first. We generalize once a second language exists to triangulate against.

## Phase 0 — Walking skeleton (week 1–2)

**Goal:** `fabrik run "echo hello"` executes, caches the result by command digest, and returns cached output on second invocation.

- Cargo workspace: `fabrik-cli`, `fabrik-core`, `fabrik-cas`.
- Local content-addressed store: blobs in `.fabrik/cas/`, action results in `.fabrik/actions/`.
- One action type: `RunCommand { argv, env, cwd }`.
- Subprocess execution, no sandbox.
- CLI: `fabrik run <cmd>`, `fabrik cache stats`.

**Exit criterion:** running the same command twice — second run is a cache hit, < 10ms.

**Explicitly deferred:** sandbox, DSL, plugins, graph, telemetry, remote.

## Phase 1 — Single-crate Rust build (week 3–5)

**Goal:** `fabrik build` produces the same binary as `cargo build` for a hello-world Rust crate, byte-identical or close to it.

- New crate `fabrik-rust` (in-process, linked into CLI).
- Reads `Cargo.toml` directly. No DSL yet.
- Invokes `cargo metadata` to get the crate graph.
- Generates one action per crate: `rustc --crate-name foo --edition 2021 ...`.
- Links Phase 0's CAS for caching.
- Inputs to cache key: source files (globbed by cargo metadata's `targets[].src_path` + a recursive glob), rustc version, feature set.

**Substrate hardening folded in from the Phase 0 review** (foundation work that is cheap to do *before* Phase 1's first non-toy consumer arrives, expensive after):

- **Streaming `Cas::get_blob` / `read_blob_to_writer`** to match the existing streaming `put_stream`. Phase 1's consumers — rustc stdout, future linker logs, dependency dumps — must not require materializing the whole blob into a `Vec<u8>`. Add the `AsyncRead` getter; keep the byte-vec convenience for tests.
- **Plugin-shaped `Action` boundary.** The current `Action` enum is closed over `RunCommand` and bakes its JSON encoding into the cache key. Before the rust plugin emits actions, decide between (a) keeping `Action` closed and lowering plugin actions to `RunCommand` primitives, or (b) opening it to `Action { kind: String, payload: bytes, declared_inputs, declared_outputs }` with plugin-defined schemas. Whichever wins becomes the published v1 contract; pick now so Phase 1's rust plugin is the first real consumer of the right shape.
- **Incremental `stats()` via a sidecar manifest.** The current implementation walks the entire CAS tree on every call — fine at 12 entries, brutal at the volumes Phase 1 produces. Track running totals in `<root>/meta.json`, updated on every `put_*` and `forget_*`, with `fabrik cache verify` as the recompute path. Keep the walking implementation behind `verify` only.

**Exit criterion:** clean cache → `fabrik build` produces a working binary. Touch a `.rs` file → only that crate's rustc runs. No edits → 100% cache hit. The three substrate items above ship in the same phase, gated by the same exit criterion.

**Risks:** rustc has many implicit inputs (env vars, lockfile, target dir layout). Expect to discover them by diffing cargo's behavior. Worth keeping a `loose` mode escape hatch from day one to unblock progress. The `Action`-shape decision is the load-bearing one — getting it wrong means Phase 4's plugin extraction is a flag-day cache invalidation.

## Phase 2 — Multi-crate parallel build (week 6–8)

**Goal:** Build a multi-crate workspace (Fabrik's own) with parallelism, dependency ordering, and incremental rebuilds.

- Action graph in `fabrik-core`: typed nodes, edges from declared deps.
- Scheduler: topological order, N-way parallel (default = num CPUs).
- Build script support via **traced mode** — Linux-first using `bpftrace` or `LD_PRELOAD` shim. macOS via `dtrace` (best-effort) or fall back to `loose` mode. This is the highest-risk item in the early plan; budget time accordingly.
- Proc macro support: separate host-platform compilation pass (no profile transitions yet — just a hardcoded host build).
- Error structure: capture `rustc --error-format=json` and surface it as the typed error from §7 of the design spec.

**Exit criterion:** `fabrik build` builds Fabrik's own workspace end-to-end. Wall-clock time on a clean cache is within 2× of `cargo build`. Incremental rebuild on a 1-line change to a leaf crate is faster than `cargo build`.

## Phase 3 — Self-hosting milestone (week 9–10)

**Goal:** Fabrik builds Fabrik *and* we use it daily.

- CI builds Fabrik with Fabrik (alongside `cargo build` as a check).
- Local dev workflow uses `fabrik build` and `fabrik test`.
- `fabrik test` added: wraps `rust_test` targets, runs binaries, captures structured output.
- Bug-fix sprint: whatever breaks during dogfooding gets fixed before moving on.

**Exit criterion:** the Fabrik team uses `fabrik build` instead of `cargo build` for at least a week without falling back. Wall-clock time on a warm cache is **better** than cargo, not just comparable.

This is the dogfood gate. Don't proceed past Phase 3 until it's met.

## Phase 4 — Extract the plugin contract (week 11–13)

**Goal:** Refactor `fabrik-rust` to live behind a typed plugin interface, still in-process. No user-visible change.

- Define `Plugin` trait + protobuf schema for: target type registration, resolution (graph fragment emission), execution (action emission), queries.
- `fabrik-rust` becomes the first implementation of this trait.
- Add a stub `fabrik-command` plugin (generic `command` target type) so we have a second implementation to triangulate on.
- The trait is the contract that WASM plugins will eventually implement. Designing it against two real implementations protects us from a contract that only fits one.

**Exit criterion:** zero user-visible regression, `fabrik-rust` compiled out cleanly works only through the trait, and the trait is stable enough to publish as an internal-but-reviewable API.

## Phase 5 — Build definition language (week 14–18)

**Goal:** Replace hardcoded `Cargo.toml` reading with `.fabrik` files. Cargo.toml becomes one *input* to the rust plugin, not the source of build truth.

- DSL parser, type checker, schema registry (every plugin contributes its target schemas).
- Bootstrap migration: the rust plugin reads `Cargo.toml` automatically when the `.fabrik` file says `kind = "rust_workspace"` — i.e., we don't make users hand-write target stanzas for Rust crates immediately.
- LSP server (basic completion + diagnostics) — agents and humans both benefit.
- Profiles: implement `profile` blocks, partition cache namespaces by profile.

**Exit criterion:** Fabrik's own build is described in `.fabrik` files; `Cargo.toml` is read by the rust plugin but no longer the build entry point. Two profiles (`debug`, `release`) work cleanly.

**Open question to resolve in this phase:** bespoke DSL vs typed-TS subset (open question #1 in the design). Prototype both early in this phase and decide.

## Phase 6 — WASM plugin host (week 19–22)

**Goal:** Move the rust plugin to WASM. Prove the contract works across the WASM boundary.

- WASI runtime (Wasmtime) integrated as `fabrik-plugin-host`.
- `fabrik-rust` compiled to a `.wasm` artifact, distributed alongside the binary.
- Plugin manifest: declared capabilities (filesystem reads, subprocess spawning), pinned digest.
- Capability negotiation: the host grants narrow filesystem and subprocess access based on the manifest, refuses everything else.

**Exit criterion:** `fabrik build` of Fabrik works with the rust plugin loaded as WASM. Performance overhead < 5% (plugin runs are dwarfed by rustc time anyway).

**Risk:** WASI Preview 2 ecosystem maturity (open question #2). May need a constrained interface that grows with the ecosystem. Have a documented fallback to in-process plugins if a specific plugin needs capabilities WASI doesn't yet expose.

## Phase 7 — REAPI substrate + remote cache (week 23–28)

**Goal:** Swap local-only CAS for a REAPI client. Connect to a remote cache. Cache hits across machines.

- REAPI client in `fabrik-cas`, replacing direct disk store (disk store stays as the local tier).
- Compatibility tested against BuildBuddy and NativeLink.
- Provenance metadata extension to REAPI action results (custom fields).
- Predicate-based invalidation: `fabrik cache invalidate --plugin=rust@1.x`.
- CI publishes cache; developers consume it. Cold-start `fabrik build` on a fresh checkout should hit cache for everything CI built.

**Exit criterion:** new contributor's first `fabrik build` of Fabrik completes in under 30 seconds via remote cache hits.

## Phase 8 — Query API + OTel telemetry (week 29–32)

**Goal:** The introspection layer that makes Fabrik agent-native.

- gRPC `GraphService` exposed by the coordinator (subset of the §6.1 API: `GetTarget`, `GetTransitiveDeps`, `Impact`, `WhyRebuild`, `ExplainCacheMiss`).
- OTel emission with the `fabrik.*` semantic conventions defined in §10.2.
- Structured event log alongside OTel for the lineage data that's too high-volume for traces.
- CLI commands (`fabrik query`, `fabrik impact`, `fabrik why-rebuild`) become thin clients over the gRPC API.
- Dogfood with Claude/agents: have them use the query API to answer questions about the Fabrik build itself. File API gaps as we find them.

**Exit criterion:** an agent can answer "what would change if I removed crate X?" and "why did action Y rebuild?" using only the query API, no source reading.

## Phase 9 — Second language (week 33–40)

**Goal:** Prove the plugin model with a non-Rust language. Choice: C/C++ or Go (open question #4).

**Recommended: Go first.** Cleaner cooperative integration (`go list -json` is excellent), simpler build model, faster path to a working second plugin. C/C++ second — it's strategically more important but technically harder (depfile handling, system libraries, hermetic toolchain bundles).

- New `fabrik-go` plugin (WASM from day one, since the host exists now).
- Cooperative resolution via `go list -json`.
- Reimplemented execution: `go tool compile` per package.
- Cross-language proof: a small Go service in the Fabrik repo that depends on a Rust-generated artifact. End-to-end caching across the language boundary.

**Exit criterion:** the cross-language sample works end-to-end, with the integration mode visible in query results, and adding Go to a Fabrik workspace requires < 50 lines of `.fabrik` config.

## Phase 10+ — Beyond v0

After Phase 9 we have what the design spec calls v0.5: self-hosting Rust + a second language + REAPI + agent-queryable. The remaining roadmap follows the design spec's §11.3 onward — TypeScript, Python, Java/Kotlin, Elixir, Windows traced mode, then the hard mobile ecosystems. Detailed sequencing for those phases gets written when we get there; planning beyond a 6-month horizon for a project this novel is fiction.

## Cross-cutting workstreams

These run in parallel with the phased work, not as separate phases:

- **Test infrastructure**: integration tests that build a real-but-small workspace end-to-end. Set up in Phase 0, expanded every phase. Without this we cannot dogfood safely.
- **Benchmarks**: wall-clock comparison vs `cargo build` on a fixed corpus. Tracked in CI from Phase 1. The self-hosting milestone (Phase 3) requires this to even exist.
- **Documentation**: keep [design.md](design.md) in sync with what's actually shipped. Mark unbuilt sections explicitly. Avoids the "specs as fiction" drift.
- **Error structure**: the typed error contract from §7 of the design gets implemented incrementally — Phase 1 ships a stub, every plugin tightens its parsing as it matures.

## Decision log: open questions and when they get answered

| # | Question | Phase to decide |
|---|---|---|
| 1 | DSL: bespoke vs typed-TS subset | Phase 5 (prototype both early in the phase) |
| 2 | WASI version + capability model | Phase 6 |
| 3 | Workspace persistence format | Phase 2 (when the graph store starts mattering) |
| 4 | First non-Rust language | Phase 9 (recommended: Go) |
| 5 | `service` targets — own supervisor or integrate? | Defer to v0.5+ |
| 6 | Query API surface | Phase 8 (deliberately narrow start, grow from agent dogfooding) |
| 7 | Plugin distribution | Phase 6 (URL+digest pinning sufficient for v0; registry later) |

## What gates progress

Three gates that must hold for the plan to stay on track:

- **Phase 3 (self-hosting)**: if we can't dogfood Fabrik on Fabrik by week 10, the architecture is wrong and we re-plan rather than push forward.
- **Phase 6 (WASM)**: if the WASI Preview 2 ecosystem isn't ready and our fallback constraints are too painful, we ship v0 with in-process plugins and move WASM to v1. The plugin *contract* still exists.
- **Phase 8 (query API + agent dogfooding)**: if agents can't usefully answer real questions via the API, the API is wrong and we redesign it. This is the agent-native bet; it has to be tested with real agents on real builds.
