# Fabrik: Execution Roadmap

A phased plan optimized for **shortest path to self-hosting**. The design spec ([design.md](design.md)) describes the destination; this document sequences how we get there.

## Sequencing principles

1. **Self-hosting is the forcing function.** Every phase is judged by how much closer it gets us to "Fabrik builds Fabrik faster than cargo." Architectural elegance that doesn't move that needle waits.
2. **Defer architectural complexity until the system works.** REAPI, the query API, and external plugin distribution are all in the v1 design but each blocks self-hosting if we build them first. We bring them in once we have a working concrete system to extract them from.
3. **Every phase ships a working artifact.** No phase is "refactor with no user-visible change." If a phase doesn't produce something runnable that's better than the prior phase, the phase is wrong.
4. **The plugin contract is in from day one.** Built-in plugins live behind the same TOML schema and Rust planner interface third-party plugins will use. We don't ship a "hardcoded Rust support, extract later" milestone, because that's the refactor we'd most regret.
5. **Declared before adopted.** The Rust plugin's declared-mode targets (`rust.binary`, `rust.library`, `rust.test`) ship first because Fabrik's own build uses them. Adopted-mode (`rust.workspace`, reads `Cargo.toml`) lands afterward as the external-adoption story.
6. **Hardcode before generalize.** Plugin-specific Rust handlers (rustc invocation, diagnostic parsing) are hardcoded at first. We generalize once a second language plugin exists to triangulate against.

## Phase 0: Walking skeleton (week 1-2)

**Goal:** `fabrik run "echo hello"` executes, caches the result by command digest, and returns cached output on second invocation.

- Cargo workspace: `fabrik-cli`, `fabrik-core`, `fabrik-cas`.
- Local content-addressed store: blobs in `.fabrik/cas/`, action results in `.fabrik/actions/`.
- One action type: `RunCommand { argv, env, cwd }`.
- Subprocess execution, no sandbox.
- CLI: `fabrik run <cmd>`, `fabrik cache stats`.

**Exit criterion:** running the same command twice: second run is a cache hit, < 10ms.

**Explicitly deferred:** sandbox, DSL, plugins, graph, telemetry, remote.

## Phase 1: TOML frontend + declared Rust (week 3-6)

**Goal:** `fabrik build //hello:hello` reads a `fabrik.toml` file with a declared `[[rust.binary]]` target and produces a working binary, with the same caching behavior the CAS already has.

- New crate `fabrik-frontend`: defines the `fabrik.toml` loader, target schemas, validation, and label resolution.
- New crate `fabrik-rust`: registers `rust.library`, `rust.binary`, `rust.test`, `rust.proc_macro`, plus Rust handlers such as `rust.rustc_invoke` and `rust.parse_diagnostics`.
- Plugin contract: validated TOML declarations are passed to plugin planners, which emit typed `Action` records that the substrate runs. Same shape that any future plugin will use.
- Generates one action per declared target. Cache key inputs: srcs digest, deps, rustc version, feature flags.
- No `Cargo.toml` reading in this phase. Targets are written by hand in `fabrik.toml` files.

**Exit criterion:** clean cache produces a working `hello` binary from a hand-written `fabrik.toml`. Touch the source, only that target's rustc runs. No edits, 100% cache hit.

**Risks:** rustc has many implicit inputs (env vars, lockfile, target dir layout). We discover them by diffing cargo's behavior on the same source. A `loose` hermeticity escape hatch ships day one to unblock progress.

## Phase 2: Multi-crate parallel build (week 7-9)

**Goal:** Build the Fabrik workspace itself end-to-end from hand-written `fabrik.toml` files, with parallelism, dependency ordering, and incremental rebuilds.

- Action graph in `fabrik-core`: typed nodes, edges from declared deps.
- Scheduler: topological order, bounded by CPU slots and optional memory budgets.
- Build script support via **traced mode**. Linux-first using `bpftrace` or `LD_PRELOAD` shim. macOS via `dtrace` (best-effort) or fall back to `loose` mode. Highest-risk item in the early plan; budget time accordingly.
- Proc macro support: separate host-platform compilation pass (no profile transitions yet, just a hardcoded host build).
- Error structure: capture `rustc --error-format=json` and surface it as the typed error from §7 of the design spec.
- Hand-written `fabrik.toml` files for each Fabrik crate. This is one-time work that doubles as the first real test of the target schema ergonomics.

**Exit criterion:** `fabrik build` builds Fabrik's own workspace end-to-end from declared targets. Wall-clock time on a clean cache is within 2x of `cargo build`. Incremental rebuild on a 1-line change to a leaf crate is faster than `cargo build`.

## Phase 3: Self-hosting milestone (week 10-11)

**Goal:** Fabrik builds Fabrik *and* we use it daily.

- CI builds Fabrik with Fabrik (alongside `cargo build` as a check).
- Local dev workflow uses `fabrik build` and `fabrik test`.
- `fabrik test` added: wraps `rust.test` targets, runs binaries, captures structured output.
- Bug-fix sprint: whatever breaks during dogfooding gets fixed before moving on.

**Exit criterion:** the Fabrik team uses `fabrik build` instead of `cargo build` for at least a week without falling back. Wall-clock time on a warm cache is **better** than cargo, not just comparable.

This is the dogfood gate. Don't proceed past Phase 3 until it's met.

## Phase 4: Adopted-mode Rust + second plugin (week 12-15)

**Goal:** Add `rust.workspace` (adopted mode) so external cargo projects can drop in. Add a second built-in plugin (`task`) to validate the plugin contract against two real implementations.

- `rust.workspace` target type: reads `Cargo.toml`, runs `cargo metadata` cooperatively, generates declared-mode targets internally. Same handlers as declared mode; cache entries kept in a separate namespace.
- `task` plugin: generic runtime task target type for ad-hoc commands. Forces us to confirm the plugin SDK is reusable, not just rust-shaped.
- Sharpen the plugin SDK based on what hurt during the second-plugin build: helper functions for action declaration, glob handling, output declaration, schema validation.

**Exit criterion:** an existing cargo workspace adopts Fabrik with one `fabrik.toml` file containing `[[rust.workspace]]` and gets cache hits across builds. The `task` plugin is usable for shell-out targets without poking inside Fabrik internals.

## Phase 5: DSL maturity: profiles, LSP, schema registry (week 16-19)

**Goal:** Make TOML build declarations production-grade for humans and agents. The language itself is settled as structured data; this phase is about schemas, diagnostics, and importers on top of it.

- Profiles: implement `[[profile]]` declarations, partition cache namespaces by profile, support `--profile` selection on the CLI.
- LSP server: completion, schema-aware diagnostics, and jump-to-target for labels. Agents and humans both benefit.
- Schema registry: every plugin contributes its target schemas, the LSP and the frontend share the same registry, errors are typed and located.
- Documentation site: every built-in target type, generated from the schemas, with examples.

**Exit criterion:** two profiles (`debug`, `release`) work cleanly with separate cache namespaces. The LSP gives schema-aware completion for `[[rust.binary]]` and friends. An agent can author a new `fabrik.toml` file from the docs without trial and error.

## Phase 6: REAPI substrate + remote cache (week 20-25)

**Goal:** Swap local-only CAS for a REAPI client. Connect to a remote cache. Cache hits across machines.

- REAPI client in `fabrik-cas`, replacing direct disk store (disk store stays as the local tier).
- Compatibility tested against BuildBuddy and NativeLink.
- Shared resource requests for local and remote actions: CPU slots, optional memory bytes, remote queue capacity, output downloads, and prefetch budget.
- Provenance metadata extension to REAPI action results (custom fields).
- Predicate-based invalidation: `fabrik cache invalidate --plugin=rust@1.x`.
- CI publishes cache; developers consume it. Cold-start `fabrik build` on a fresh checkout should hit cache for everything CI built.

**Exit criterion:** new contributor's first `fabrik build` of Fabrik completes in under 30 seconds via remote cache hits.

## Phase 7: Query API + OTel telemetry (week 26-29)

**Goal:** The introspection layer that makes Fabrik agent-native.

- gRPC `GraphService` exposed by the coordinator (subset of the §6.1 API: `GetTarget`, `GetTransitiveDeps`, `Impact`, `WhyRebuild`, `ExplainCacheMiss`).
- OTel emission with the `fabrik.*` semantic conventions defined in §10.2.
- Structured event log alongside OTel for the lineage data that's too high-volume for traces.
- CLI commands (`fabrik query`, `fabrik impact`, `fabrik why-rebuild`) become thin clients over the gRPC API.
- Dogfood with Claude/agents: have them use the query API to answer questions about the Fabrik build itself. File API gaps as we find them.

**Exit criterion:** an agent can answer "what would change if I removed crate X?" and "why did action Y rebuild?" using only the query API, no source reading.

## Phase 8: Second language (week 30-37)

**Goal:** Prove the plugin model with a non-Rust language. Choice: C/C++ or Go (open question #2).

**Recommended: Go first.** Cleaner cooperative integration (`go list -json` is excellent), simpler build model, faster path to a working second plugin. C/C++ second: strategically more important but technically harder (depfile handling, system libraries, hermetic toolchain bundles).

- New `fabrik-go` plugin: schemas, planners, and Go-specific Rust handlers, same shape as the Rust plugin.
- Declared mode (`go.binary`, `go.library`, `go.test`) and adopted mode (`go.module`, reads `go.mod`) from the start, since both patterns are settled by now.
- Cooperative resolution via `go list -json` for adopted mode.
- Reimplemented execution: `go tool compile` per package.
- Cross-language proof: a small Go service in the Fabrik repo that depends on a Rust-generated artifact. End-to-end caching across the language boundary.

**Exit criterion:** the cross-language sample works end-to-end, with the integration mode visible in query results, and adding Go to a Fabrik workspace requires < 50 lines of `fabrik.toml` config.

## Phase 9+: Beyond v0

After Phase 8 we have what the design spec calls v0.5: self-hosting Rust + a second language + REAPI + agent-queryable. The remaining roadmap follows the design spec's §11.3 onward: TypeScript, Python, Java/Kotlin, Elixir, Windows traced mode, then the hard mobile ecosystems. Detailed sequencing for those phases gets written when we get there; planning beyond a 6-month horizon for a project this novel is fiction.

Third-party plugin distribution (handler subprocess protocol, URL+digest pinning) lands somewhere in this window, driven by the first credible third-party plugin author.

## Cross-cutting workstreams

These run in parallel with the phased work, not as separate phases:

- **Test infrastructure**: integration tests that build a real-but-small workspace end-to-end. Set up in Phase 0, expanded every phase. Without this we cannot dogfood safely.
- **Benchmarks**: wall-clock comparison vs `cargo build` on a fixed corpus. Tracked in CI from Phase 1. The self-hosting milestone (Phase 3) requires this to even exist.
- **Documentation**: keep [design.md](design.md) in sync with what's actually shipped. Mark unbuilt sections explicitly. Avoids the "specs as fiction" drift.
- **Error structure**: the typed error contract from §7 of the design gets implemented incrementally. Phase 1 ships a stub, every plugin tightens its parsing as it matures.

## Decision log: open questions and when they get answered

| # | Question | Phase to decide |
|---|---|---|
| 1 | Workspace persistence format | Phase 2 (when the graph store starts mattering) |
| 2 | First non-Rust language | Phase 8 (recommended: Go) |
| 3 | `service` targets: own supervisor or integrate? | Defer to v0.5+ |
| 4 | Query API surface | Phase 7 (deliberately narrow start, grow from agent dogfooding) |
| 5 | Third-party plugin distribution + handler protocol | Phase 9+ (driven by first credible third-party plugin) |

### Resolved (kept here for historical reference)

- **Build definition language**: TOML declarations validated against plugin schemas. Decided after comparing Starlark, Pkl, and TypeScript against agent authoring and validation workflows.
- **Plugin implementation**: first-party Rust planners plus named runtime handlers. WASM is deferred until a third-party plugin author needs untrusted-code isolation.

## What gates progress

Three gates that must hold for the plan to stay on track:

- **Phase 3 (self-hosting)**: if we can't dogfood Fabrik on Fabrik by week 11, the architecture is wrong and we re-plan rather than push forward.
- **Phase 4 (second plugin)**: if the plugin SDK requires substantial reshaping to fit a non-Rust plugin, the contract is wrong and the SDK gets a redesign before Phase 5 ships.
- **Phase 7 (query API + agent dogfooding)**: if agents can't usefully answer real questions via the API, the API is wrong and we redesign it. This is the agent-native bet; it has to be tested with real agents on real builds.
