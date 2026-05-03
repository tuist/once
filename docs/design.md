# Fabrik: Design Specification (v0 draft)

A polyglot, agent-native build system. Bazel's ambitions, none of its mistakes.

## 0. Goals and non-goals

### Goals

- One build system for the entire polyglot stack: C, C++, Rust, Go, Elixir, Java/Kotlin, Python, TypeScript/JavaScript, Swift/Objective-C. No language is a second-class citizen long-term.
- Trustworthy, content-addressed, remote-shareable caching for every action, with honest boundaries where fidelity must degrade.
- The build graph is a typed, queryable data structure. Agents and humans interact with the same API.
- Causal, structured errors. Agents can act on errors programmatically; humans get useful messages.
- Hermetic execution by default, with explicit, visible escape hatches when hermeticity isn't achievable.
- Rust-first to enable dogfooding from day one, but the architecture is language-agnostic from the start.

### Non-goals

- Replacing every native build tool. We coordinate with cargo, mix, gradle, xcodebuild, vite where reimplementation is worse than cooperation.
- A novel programming language for build files. We learned from Starlark.
- Shipping a UI in v0. The OTel ecosystem already has them.
- Compatibility with Bazel BUILD files. We learn from Bazel; we don't carry its baggage.

### What success looks like

- Fabrik builds itself (Rust + some C dependencies) faster and more reproducibly than `cargo build` within 6 months.
- An agent can answer "why did this rebuild?" and "what would change if I removed this dep?" without reading source code.
- A polyglot backend monorepo (Rust + Go + TypeScript + Python + protobuf) can adopt Fabrik in a week and see remote cache hits across all languages.
- iOS, Android, and complex Gradle setups have a clear, honest story: not as good as Bazel-with-`rules_apple` on day one, on a credible path to parity.

## 1. The mistakes we are not repeating

This section exists because every architectural decision below is a reaction to a specific Bazel pain point. Naming them up front keeps us honest.

| Bazel mistake | Our response |
|---|---|
| Untyped Starlark, weakly-typed providers, sprawling rule complexity | Typed Starlark via `starlark-rust` (Buck2 dialect); typed providers and target schemas; one extension concept |
| Four overlapping extension mechanisms (macros, rules, aspects, repository rules) | One concept: a plugin is a pure function that emits typed graph fragments |
| BUILD files require manual enumeration of every source and dep | Globs and language-aware discovery are first-class, with deterministic resolution |
| WORKSPACE → MODULE.bazel migration, still ongoing | One module file from day one. Versioned. Stable. |
| `rules_*` ecosystem perpetually lags upstream language toolchains | Cooperative integration with native tools is the default; reimplementation is opt-in |
| Untyped providers, schema-by-convention | Typed plugin contracts validated at the boundary |
| Hermeticity is aspirational; cache poisoning is silent | Hermeticity level is per-target, declared, stored in cache, queryable |
| Error messages span Starlark/generated code/execution; root cause is buried | Structured errors with full provenance; "why" is queryable |
| `bazel query` is bolted on, action graph is internal | Query API is the primary interface; CLI is a thin client |
| `--config` flag combinatorics produce non-deterministic caching | Configuration profiles are typed, named, part of the cache key namespace |
| Long-running dev servers shoehorned into build target model (`ibazel`) | `service` targets are a separate first-class concept |
| iOS/Android support is heroic third-party effort | First-party plugin teams for hard ecosystems, planned and resourced |
| Remote execution requires Bazel; protocol is Bazel-shaped | REAPI-compatible at the wire level; we benefit from existing remote backends |
| Build telemetry is BEP, proprietary tooling on top | OTel-native with build-specific semantic conventions |

## 2. Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│  Frontend                                                        │
│   • Build definition language (typed Starlark)                   │
│   • Plugin host (starlark-rust, in-process)                      │
├──────────────────────────────────────────────────────────────────┤
│  Coordinator                                                     │
│   • Graph model (typed, persistent, queryable)                   │
│   • Scheduler (local + remote workers)                           │
│   • Query API (gRPC, primary interface)                          │
│   • Telemetry router (OTel + structured event log)               │
│   • Error provenance store                                       │
├──────────────────────────────────────────────────────────────────┤
│  Substrate                                                       │
│   • Content-addressed storage (CAS), REAPI-compatible            │
│   • Sandbox (strict / traced / loose)                            │
│   • Action execution (local + remote)                            │
│   • Action cache with provenance metadata                        │
└──────────────────────────────────────────────────────────────────┘
```

Three layers, clean interfaces, each independently testable. The coordinator is the brain; the substrate is the muscle; the frontend is the interface.

### 2.1 Substrate

REAPI-compatible at the wire level so we inherit BuildBuddy, Buildbarn, NativeLink, EngFlow as remote backends. Internally we add:

- **Provenance metadata on every cache entry**: producing plugin name+version, hermeticity level, platform, timestamp, action key inputs.
- **Predicate-based invalidation**: `fabrik cache invalidate --plugin=rust@1.2.3 --platform=linux/arm64`. Bazel's "wipe everything" is not the only option.
- **Three sandbox modes**:
  - `strict`: declared inputs only, no network, hermetic env. Default for reimplemented plugins.
  - `traced`: syscall tracing (FUSE on Linux, EndpointSecurity on macOS, ETW on Windows) discovers actual inputs on first run; subsequent runs use that as the cache key. Default for cooperative plugins.
  - `loose`: runs in workspace, trusts user-declared inputs. Required acknowledgment in target definition. Required for most opaque-mode targets.
- **User-defined cache keys**: the substrate stores and retrieves by digest; the plugin owns what goes into the digest. This is critical: cargo wants `Cargo.lock + features + rustc version`; vite wants `package-lock.json + vite.config + source digest`. One-size-fits-all keys are why Bazel struggles to integrate with native tools.

### 2.2 Coordinator

Stateless service. Build graph state lives in the workspace (`.fabrik/`); action results live in the substrate. The coordinator is rebuilt from these on every invocation.

Owns:
- The typed action graph (protobuf schema).
- The scheduler (topological order, parallelism, remote dispatch).
- The query API (see §6).
- The telemetry router (OTel exporter + structured event log).
- The error provenance store (queryable history of failures).

### 2.3 Frontend

The build definition language and plugin host. Build definitions and plugins are both written in typed Starlark, evaluated in-process via `starlark-rust` (the implementation Buck2 uses). Build files are pure data. Plugins are pure functions from typed inputs to typed action graphs. There is no other way to extend Fabrik.

The plugin author writes Starlark; impure work (subprocess execution, output parsing, depfile handling) lives in Rust handlers in the substrate that the plugin references by name. The Starlark side stays deterministic by construction; the substrate is responsible for hermetic execution of the actions the plugin declares.

## 3. Build definition language

### 3.1 Core

Typed Starlark, evaluated in-process via `starlark-rust`. The Buck2 dialect with type annotations on all functions and providers. Reference points: Buck2's prelude, Bazel BUILD files (the surface a typical user writes, not the rule-author surface).

A `fabrik.star` file looks like this (each package directory holds one; plugin and SDK modules use the same `.star` extension):

```python
load("//build/rust.star", "rust_binary", "rust_test")

rust_binary(
    name = "api",
    srcs = glob(["src/**/*.rs"]),
    deps = [
        "//lib/core",
        "//lib/proto:rust",
        "@crates//tokio",
    ],
    visibility = ["//services/..."],
)

rust_test(
    name = "api_test",
    srcs = glob(["tests/**/*.rs"]),
    deps = [":api", "@crates//tokio-test"],
)
```

The language has:
- Static typing on plugin-declared target functions (a plugin's `rust_binary` declares `srcs: list[str]`, `deps: list[str]`, etc., and the evaluator enforces it).
- Typed providers (records produced by one target and consumed by another).
- Imports via `load("//path/to/file.star", "symbol")`.
- Pure functions for data transformation (`glob`, `select`, `map`, `filter`).
- No mutable state at module scope, no I/O at evaluation time, no recursion past Starlark's bounded recursion limit.

### 3.2 Computation: Starlark plugins

When you need real computation, generating N targets from a manifest, polyglot stitching, conditional logic beyond what `select` covers, you write a plugin. A plugin is a Starlark module that defines target types (functions that take typed kwargs and emit graph nodes) and exports them via a `fabrik_plugin(...)` declaration.

Plugins are pure Starlark. They cannot do I/O, spawn subprocesses, or read arbitrary files. What they can do:

- Declare target types with typed schemas.
- Compute target graphs from typed inputs (including the result of `glob`, which the host evaluates against the workspace as a typed primitive).
- Emit `Action` records (typed: `command`, `inputs`, `outputs`, `env`, `cache_key_extras`) that the substrate runs.
- Reference Rust handlers in the substrate by name when an action requires plugin-specific runtime work (parsing rustc JSON output, depfile handling, swiftc module map computation). The Starlark side names the handler; the Rust side implements it. Bundled handlers ship with the plugin.

This replaces Bazel's macros, rules, aspects, and repository rules with **one concept**: a typed Starlark function that emits a typed graph fragment.

### 3.3 Configuration profiles

```python
profile(
    name = "release_linux_x64",
    platform = "linux/x86_64",
    rust = rust_settings(toolchain = "stable-1.78", opt_level = 3, lto = "thin"),
    c = c_settings(toolchain = "clang-18", opt_level = 3, sanitizers = []),
)

profile(
    name = "debug_macos_arm64",
    platform = "macos/aarch64",
    rust = rust_settings(toolchain = "stable-1.78", opt_level = 0, debug = True),
)
```

A profile is a typed bundle of toolchain selections, build settings, and platform constraints. Replaces Bazel's `--config` sprawl, transitions, toolchains, and platforms with one concept.

Cache namespaces are partitioned by profile. Same inputs + same profile → same output, always. Different profile → different cache entry, no collision.

## 4. Plugin model

A plugin is a Starlark module that declares typed target functions and registers them with `fabrik_plugin(...)`. The module is loaded into the same `starlark-rust` evaluator that loads user `fabrik.star` files; there is no host/plugin boundary at the language level.

```python
load("//fabrik/sdk.star", "fabrik_plugin", "resolution", "execution")

# Declared-mode entry points. Users write these directly in fabrik.star files.
def rust_binary(name, srcs, deps = [], visibility = None, ...):
    return _rust_target(kind = "binary", name = name, srcs = srcs, deps = deps, ...)

def rust_library(name, srcs, deps = [], visibility = None, ...):
    return _rust_target(kind = "library", name = name, srcs = srcs, deps = deps, ...)

# Adopted-mode entry point. Reads Cargo.toml, generates declared-mode targets internally.
def rust_workspace(name, manifest = "Cargo.toml", ...):
    return _rust_workspace(name = name, manifest = manifest, ...)

fabrik_plugin(
    name = "rust",
    version = "1.0.0",
    target_types = {
        "rust_binary": rust_binary,
        "rust_library": rust_library,
        "rust_test": rust_test,
        "rust_proc_macro": rust_proc_macro,
        "rust_workspace": rust_workspace,
    },
    # Declared-mode targets are reimplemented; adopted-mode targets are cooperative.
    integration = {
        "rust_binary": execution(mode = "reimplemented", sandbox = "strict"),
        "rust_library": execution(mode = "reimplemented", sandbox = "strict"),
        "rust_test": execution(mode = "reimplemented", sandbox = "strict"),
        "rust_proc_macro": execution(mode = "reimplemented", sandbox = "strict"),
        "rust_workspace": resolution(mode = "cooperative", tool = "cargo",
                                     inputs = ["Cargo.toml", "Cargo.lock"]),
    },
    cache_key_inputs = ["srcs", "deps", "rustc_version", "features", "profile.rust"],
    queries = ["transitive_crates", "feature_set", "build_script_outputs"],
    handlers = ["rust.rustc_invoke", "rust.parse_diagnostics", "rust.cargo_metadata"],
)
```

The `handlers` field names Rust handlers in the substrate that the plugin's actions reference. Bundled handlers ship with the plugin distribution.

### 4.1 The three integration modes

These are not aspirational labels; they are typed contracts that determine cache fidelity, query depth, error structure, and sandbox defaults.

**`reimplemented`**: plugin emits explicit actions with declared inputs and outputs. Maximum cache fidelity. Used where the compilation unit maps cleanly to a single tool invocation: rustc per crate, gcc/clang per .c/.cpp, javac per source set, swiftc per module.

**`cooperative`**: plugin uses the native tool to *discover* the graph but executes via the substrate. The plugin runs `cargo metadata` or `go list -json` or `mix xref` to get a structured dep graph, then translates it into Fabrik targets. Execution can be either reimplemented (we call rustc directly) or opaque (we run `mix compile`).

**`opaque`**: one target = one tool invocation. Cache key is the input digest + tool version + flags. Fine-grained queries past this boundary return `{"boundary": "opaque", "tool": "...", "reason": "..."}`. Used where reimplementation is hopeless: Vite production builds, Gradle projects, Mix applications, xcodebuild.

For most language plugins, both modes coexist within the same plugin: a "declared" target type (reimplemented, full graph control) and an "adopted" target type (cooperative, points at the upstream manifest). See §9.1 for how this looks for Rust.

### 4.2 Honest boundaries

The integration mode is **visible to users and agents**, surfaced in:
- The build definition (`kind = "gradle_project"` is implicitly opaque; we make this loud).
- The query API (`fabrik introspect //app/android` returns the boundary type).
- Marketing materials and docs (we don't pretend Gradle gets the same treatment as Rust).
- Cache statistics (`fabrik stats --by-mode` shows hit rates broken down by integration mode).

This is the single biggest cultural difference from Bazel. Bazel claims total knowledge; we explicitly mark where knowledge ends.

### 4.3 Plugin distribution and versioning

- Plugin API has semver. Workspaces declare plugin versions. Breakage is detected, not silent.
- A plugin is a Starlark module plus a set of Rust handlers. Built-in plugins (rust, c, go, ...) ship inside the Fabrik binary; their Starlark sources are embedded and their handlers compiled in.
- Third-party plugins ship as a (Starlark module, handler binary) bundle pinned by URL+digest. The handler binary is invoked by the substrate as a long-lived subprocess speaking a typed protocol (this is the only IPC boundary in the system; the Starlark layer never leaves the host process). Registry-based distribution is a follow-up to URL+digest pinning.
- Plugin schemas (target types, providers, query methods) are versioned independently of plugin code.
- A target compiled with plugin v1.2 can be cache-shared with a target compiled with plugin v1.3 only if the plugin declares schema compatibility. Otherwise, separate cache namespaces.

## 5. Caching and hermeticity

### 5.1 Hermeticity is per-target, declared, queryable

```fabrik
target "build_with_system_lib" {
  kind = "c_binary"
  srcs = ["main.c"]
  hermeticity = "loose"  # explicitly opted in; we link against /usr/lib/libfoo
}
```

Every cache entry stores its hermeticity level. `fabrik why-cached //x` tells you whether the result came from a `strict`, `traced`, or `loose` cache hit. Agents can filter cache hits by hermeticity for high-reliability operations.

### 5.2 Traced mode mechanics

For `cooperative` plugins where pre-declaring inputs is impossible, the substrate runs the action in a sandbox that records every file read, syscall, and environment variable access. The recorded set becomes the action's effective input set; the cache key is computed from it.

- Linux: FUSE-based sandbox, fallback to bpftrace.
- macOS: EndpointSecurity framework (requires entitlement), fallback to dtrace.
- Windows: ETW-based monitoring.

Traced mode has a real cost: ~10-30% overhead on first build, near-zero on cache hits. Worth it for cargo's build scripts, mix's macro expansion, vite's dynamic import resolution.

### 5.3 Cache provenance and recovery

Every cache entry is annotated with: producing plugin name+version, hermeticity level, platform, action key inputs hash, timestamp, build invocation ID.

Operators can:
- Invalidate by predicate (plugin, version range, platform, age).
- Audit cache hits (`fabrik cache audit //x` shows the chain of inputs).
- Pin specific results as known-good or known-bad.

## 6. Introspection: the agent-native interface

This is the load-bearing differentiator. The query API is **not** a debugging tool bolted on at the end. It is the primary interface; the CLI is a thin client over it.

### 6.1 Query API

gRPC service exposed by the coordinator. Stable, versioned, typed. Examples:

```
GraphService.GetTarget(label) → Target with full metadata
GraphService.GetTransitiveDeps(label, depth) → DAG fragment
GraphService.GetReverseDeps(label) → which targets depend on this
GraphService.GetActionsForTarget(label) → action list with cache state
GraphService.WhyRebuild(label) → causal trace
GraphService.Impact(file_path) → affected targets with reasons
GraphService.Diff(build_id_1, build_id_2) → action-level diff between builds
GraphService.ExplainCacheMiss(action_id) → structured reason
GraphService.GetErrorHistory(label, time_range) → past failures with provenance
GraphService.PredictRebuildCost(file_changes) → estimated wall time + actions
```

Plugin-extensible: the Rust plugin registers `RustPluginService.GetTransitiveCrates(label)`, the Go plugin registers `GoPluginService.GetModulePath(label)`, etc. Plugins extend the query surface, never the CLI surface directly.

### 6.2 Designed for agents

What makes this agent-native rather than human-native with an API afterthought:

- **Every response is structured.** No "scrape stderr" paths.
- **Every response includes provenance.** The agent always knows where data came from, when, with what plugin version.
- **Queries are composable.** `Impact(file) → ExplainCacheMiss(action)` is a normal usage pattern, not a hack.
- **Streaming where appropriate.** Long-running queries (full graph dump) stream; the agent can process incrementally.
- **Predictive queries.** `PredictRebuildCost` lets an agent decide whether to make a speculative change without actually building.
- **Read-only by default.** Agents query without side effects. Mutations (cache invalidation, pinning) are separate, audited operations.

### 6.3 Agent-driven graph optimization

The query API is rich enough that an agent can:
- Identify hot paths (targets that rebuild frequently).
- Find redundant deps (targets that depend on libraries they don't use, via cross-referencing query results with source analysis).
- Suggest target splits (a target that's frequently partially invalidated could become two targets).
- Detect non-determinism (cache misses that should be hits, traced via input diffing).
- Recommend hermeticity upgrades (loose → traced → strict) based on observed input sets.

These are not features Fabrik ships; they are **uses of the API that agents naturally perform**. The bet: if the API is good enough, agent-driven optimization emerges.

## 7. Errors

Structured first, formatted second. Every error is a typed object:

```json
{
  "kind": "compile_error",
  "target": "//api:server",
  "action_id": "act_8a3f...",
  "build_id": "build_2026_04_28_001",
  "command": ["rustc", "--edition=2021", "..."],
  "exit_code": 1,
  "stderr": "...",
  "diagnostic": {
    "file": "src/main.rs",
    "line": 42,
    "column": 8,
    "code": "E0308",
    "message": "mismatched types"
  },
  "provenance": {
    "plugin": "rust@1.2.3",
    "hermeticity": "strict",
    "platform": "linux/x86_64",
    "profile": "debug_linux_x64",
    "invocation_args": ["build", "//api:server"]
  },
  "related_actions": ["act_7b2c...", "act_9d4e..."]
}
```

Plugins parse tool output into this structure where they can. Where they can't (opaque mode for Gradle), the structure still exists with raw stderr; the shape is consistent for agents.

The CLI renders these as human-readable; agents consume the JSON directly.

## 8. Long-running processes

Dev servers are not builds. They are supervised processes with their own incremental logic. Modeling them as `service` targets:

```fabrik
service "frontend_dev" {
  kind = "vite_dev"
  workspace = "//apps/web"
  watches = glob("apps/web/src/**")
  port = 5173
  depends_on_build = ["//apps/web:assets"]
}
```

The coordinator launches and supervises; it does not own the dev tool's incremental graph. Vite owns its own world inside the service; we own the supervision and the cross-language deps that feed it.

## 9. Per-language plans

This section is honest about variance. Same architecture; different fidelities.

### 9.1 Rust (v0, dogfood from day one)

The Rust plugin exposes both a declared-mode and an adopted-mode entry point. Same plugin, two faces.

**Declared mode** (`rust_binary`, `rust_library`, `rust_test`, `rust_proc_macro`). The Bazel/Buck2 posture: targets are declared in `fabrik.star` files, Fabrik is the source of truth, hermeticity defaults to `strict`.

- **Resolution**: reimplemented. The dependency graph comes from declared `deps` in the target stanza.
- **Execution**: reimplemented. Direct `rustc` invocations with `--extern` per dep.
- **Build scripts**: traced mode where they exist; explicit `build_script` target type so they show up in the graph.
- **Proc macros**: separate host-platform compilation pass via a profile transition.
- **What it's good for**: Fabrik's own build; new Rust projects that want full graph control; cross-language stitching where the typed graph matters.

**Adopted mode** (`rust_workspace`). The drop-in posture: point at a `Cargo.toml`, get caching immediately.

- **Resolution**: cooperative via `cargo metadata`. Reuse cargo's resolver, feature unification, registry. The plugin generates declared-mode targets internally; the user never sees them.
- **Execution**: reimplemented (same handlers as declared mode), or opaque (`cargo build` per crate) if the user opts out for compatibility reasons.
- **Build scripts**: traced. Cargo's build scripts are the canonical case for traced mode.
- **What it's good for**: existing cargo workspaces adopting Fabrik without migration; `crates.io` dep graphs where reproducing cargo's resolver in declared form is impractical.

**Mode boundary.** Declared and adopted targets coexist in the same workspace and the same graph. The integration mode is visible in queries (`fabrik introspect //x` reports it), in `fabrik cache stats --by-mode`, and in cache namespacing (the same crate built two ways produces two cache entries; no accidental sharing).

**Why Rust first**: we're writing Fabrik in Rust, so we get to dogfood from week one. Self-hosting uses declared mode end-to-end; adopted mode lands as the adoption story for external projects.

### 9.2 C and C++

- **Resolution**: explicit. C/C++ has no canonical metadata format; we ask users to declare deps. Compile commands, include paths, and link order are first-class typed fields.
- **Execution**: reimplemented. Direct `clang`/`gcc` invocations.
- **Header dependencies**: discovered via compiler depfile output (`-MD`), folded into the cache key. This is what every serious C build system does.
- **System libraries**: `loose` hermeticity for unavoidable system-lib linking. Hermetic toolchain bundles (LLVM + sysroot) available as a profile option for stricter builds.
- **CMake integration**: opaque-mode plugin for projects that can't migrate. `cmake --build` runs in a sandbox; outputs cached.
- **Why early**: many serious C/C++ shops are exactly the audience that wants Bazel-quality caching but bounces off Bazel's complexity. This is a strong fit.

### 9.3 Go

- **Resolution**: cooperative via `go list -json`. Maps cleanly to Fabrik targets.
- **Execution**: reimplemented. Direct `go tool compile` invocations, or `go build` per package in cooperative mode for simplicity.
- **CGo**: traced mode for the C portion.
- **Build tags**: configuration profiles handle this.
- **Confidence**: high. Go's build model is simpler than Rust's; this should Just Work.

### 9.4 TypeScript / JavaScript

- **Resolution**: cooperative via `package-lock.json` / `pnpm-lock.yaml`. Plugin generates targets per package.
- **Execution**: depends on the tool.
  - `tsc` compilation: reimplemented.
  - `vite build`, `webpack`, `esbuild` bundles: opaque, by tool invocation.
  - `vite` dev server: `service` target.
- **Honest limitation**: bundlers are not amenable to fine-grained caching by us. We cache at the bundler-invocation level. That's still a win over no caching.

### 9.5 Java / Kotlin

- **Direct compilation (small projects)**: reimplemented via `javac` / `kotlinc`. Target = compilation unit.
- **Gradle projects**: opaque. `gradle build` runs in a sandbox; outputs cached at the project level. Gradle stays in charge of its world; Fabrik handles cross-tool deps.
- **Gradle Tooling API integration**: planned for v2: finer-grained discovery without reimplementation. Research-grade.
- **Honest limitation**: Gradle users opting in to Fabrik for Gradle-specific gains will be disappointed in v0–v1. The win is *cross-language*: their Gradle project depending on protobuf generated from a different language, all coherent in one cache.

### 9.6 Python

- **Resolution**: cooperative via `pyproject.toml` + lockfile (uv, poetry, pip-tools).
- **Execution**: largely opaque. Python doesn't compile in a meaningful way; we cache the *resolved virtualenv* and the outputs of test/lint/typecheck runs.
- **`py_test`, `py_lint`, `py_typecheck`**: reimplemented as discrete actions with their own cache keys.

### 9.7 Elixir

- **Resolution**: cooperative via `mix xref` for cross-application graphs.
- **Execution**: opaque per OTP application. One application = one `mix compile`.
- **Why opaque**: Mix's incremental compilation is sophisticated and macro-aware in ways no external tool reproduces. Reimplementation is a multi-year project that nobody has finished. Opaque mode at the application boundary is honest and useful.
- **What users get**: cross-application caching (touched app rebuilds; untouched apps reuse cache), cross-language deps (Elixir app depends on protobuf from Go service), uniform telemetry. *Not* finer-grained than mix already provides.

### 9.8 Swift / iOS / macOS

This is the hard one and we're not pretending otherwise.

- **Library targets (no signing, no asset compilation)**: reimplemented via direct `swiftc`. Works for Swift Package Manager-style libraries.
- **Application targets**: opaque via `xcodebuild`. Cache by workspace digest + xcodebuild args + Xcode version.
- **Code signing**: explicitly out of cache. Signed artifacts are not cacheable across machines (different certificates, profiles). Signing happens as a post-cache step.
- **Asset compilation, plist processing, Storyboard/XIB compilation**: reimplemented per-tool over time, as the team gains capacity.
- **First-party `apple` plugin team**: required, multi-engineer, multi-year. This is a `rules_apple`-equivalent effort; we're not understating it.
- **v0 reality**: opaque mode only. Maybe 30% of what Bazel-with-`rules_apple` provides. Honest about it.
- **v2+ trajectory**: meaningful reimplementation, parity-class with `rules_apple` on a 2-3 year horizon if we resource it. Earlier if we partner with an existing iOS-on-Bazel team.

### 9.9 Android

- **Pure JVM modules**: same as Java/Kotlin above.
- **Android resource compilation, dexing, packaging**: opaque via Gradle in v0–v1. Reimplemented in v2+ behind a first-party `android` plugin.
- **Same honest framing as iOS**: hard ecosystem, multi-year effort to fully embrace, opaque mode is the v0 story.

## 10. Telemetry

OTel as the wire format, with build-specific semantic conventions on top. (Conventions defined in §10.2.)

### 10.1 What gets emitted

- **Action spans**: digest, input root digest, output digests, cache state, exit code, plugin name+version, hermeticity level, platform.
- **Build invocation root span**: command, args, profile, total wall time, cache hit rate, action counts.
- **Cache hits**: not spans (would inflate counts catastrophically): counters and attributes on the consuming span.
- **Plugin internal spans**: plugins can emit their own spans within their actions (compile, link, codegen sub-steps).
- **Structured event log**: alongside OTel, a separate log carries the full graph and result data: the BEP-equivalent. Spans are for performance; the event log is for correctness/lineage. Cross-referenced by IDs.

### 10.2 Fabrik semantic conventions for OTel

We define and publish OTel semantic conventions for build events. (Once we have working code, push these upstream as a `build` semantic convention working group.)

Key attribute namespaces: `fabrik.action.*`, `fabrik.target.*`, `fabrik.cache.*`, `fabrik.plugin.*`, `fabrik.hermeticity.*`.

### 10.3 Sharding

One trace per build invocation will exceed backend limits for any non-trivial build. Default behavior:
- Root span per invocation.
- Per-target sub-traces linked to the root.
- Per-action spans within target traces.
- Configurable thresholds for further sharding.

## 11. What we explicitly will not do

- Reimplement Vite, Webpack, esbuild. We integrate opaquely.
- Reimplement Gradle. We integrate opaquely with cooperative discovery in v2.
- Provide our own remote-execution backend. We use existing REAPI servers (BuildBuddy, Buildbarn, NativeLink).
- Ship a UI in v0–v1. The OTel ecosystem provides this.
- Bazel BUILD compatibility. We learn from Bazel; we don't import its surface area.

## 12. Open questions

These need resolution before we cut v0. I have leanings on each but want them debated.

1. **Workspace persistence format.** SQLite for the local graph index, flat files for the cache, content-addressed blobs in the substrate. But details (schema versioning, cross-machine sync of local index) need work.

2. **First non-Rust dogfooding language.** C/C++ is the strategic choice (large addressable audience, fits architecture well). Go is the easy choice (cleanest cooperative integration). Pick one for v0.5 focus.

3. **Should `service` targets exist at all?** Process supervision is its own world (systemd, pm2, mprocs, overmind). We might be better off integrating with one of these rather than building our own. Lean: build our own thin supervisor in v0, plan integration with external supervisors in v1.

4. **Agent-native query API surface.** §6 is a starting point, not exhaustive. We need to dogfood with real agents (including the one we're talking to right now) to find what's missing. v0 should ship a deliberately narrow API and grow it from observed need.

5. **Third-party plugin distribution.** Built-in plugins ship in the binary. For third-party plugins: registry vs URL+digest vs vendored. Lean: URL+digest pinning in v0 (simple, secure), registry once we have more than a handful of external plugins. The handler subprocess protocol also needs to be specified before third-party plugins are realistic.

### Resolved

- **Build definition language.** Typed Starlark via `starlark-rust` (Buck2 dialect). The earlier Pkl/TypeScript debate was settled by the realization that most of Fabrik's differentiation is structural, not language-level: typed providers, one extension concept, integration-mode boundaries, the typed query API. Starlark gives us all of that with no runtime to ship and a working precedent in Buck2.
- **Plugin isolation.** Plugins are pure Starlark evaluated in-process; impure work lives in named Rust handlers in the substrate. WASM was considered as a sandboxing mechanism but does not earn its weight: build-system plugins are mostly data shaping (Starlark covers it) plus subprocess execution (which the substrate sandboxes regardless of who declared the action). The "untrusted plugin" story is deferred until third-party plugins exist.

## 13. Why this works

The bet is structural:

- **Cooperative-first means we ship per-language support fast.** No more `rules_*` death march. Adopted-mode targets give projects a path in without rewriting.
- **One language for build files and plugins, with no runtime to ship.** Typed Starlark via `starlark-rust` keeps the binary lean, the surface small, and the agent-fluency story tractable.
- **Honest boundaries mean users know what they're getting.** No silent cache poisoning, no surprise gaps. Integration mode is queryable, not buried in docs.
- **REAPI compatibility means we inherit infrastructure.** We don't build a remote-exec backend.
- **Agent-native query API means we ride the wave.** As agents become real consumers of build telemetry, which they will in this project's lifetime, we have the surface they need. Bazel doesn't.
- **Rust-first dogfooding means we feel every paper cut.** No build system has ever been good without its authors using it daily.

The risk is also structural: ambitious scope, multi-year arc, requires sustained investment in the hard ecosystems (iOS, Android, Gradle) that won't pay off for years. We mitigate by sequencing: backend polyglot first, hard mobile/Gradle ecosystems after we've earned the right to attempt them.

---

## Appendix A: Glossary

- **Action**: a single executable unit with declared inputs, outputs, and command. The substrate caches at this level.
- **Target**: a user-facing unit defined in build files. Compiles to one or more actions.
- **Plugin**: a Starlark module (plus optional Rust handlers in the substrate) that defines target types and translates them to actions.
- **Profile**: a typed bundle of toolchain selections, build settings, and platform constraints.
- **Hermeticity level**: `strict` / `traced` / `loose`, declared per target, stored on cache entries.
- **Integration mode**: `reimplemented` / `cooperative` / `opaque`, declared per plugin per target type.
- **Substrate**: the bottom architectural layer (CAS, sandbox, action exec, cache).
- **Coordinator**: the middle architectural layer (graph, scheduler, query, telemetry).
- **Frontend**: the top architectural layer (definition language, plugin host).
- **REAPI**: Bazel's Remote Execution API. We are wire-compatible.
- **OTel**: OpenTelemetry. Our telemetry wire format.

## Appendix B: References and prior art

- Bazel: what we learn from and react against.
- Buck2 (Meta): the closest existing system in spirit; Rust-implemented, sound type system, action graph as data.
- Pants: REAPI client, good Python/JVM story.
- BuildStream: non-Bazel REAPI client, good design references.
- Pkl (Apple), Cue: typed configuration language references for the build definition surface.
- Nix: content-addressed everything; reference for hermeticity and reproducibility.
- Turborepo, Nx: what cooperative-mode caching looks like for JS; what *not* to do (silent input mis-declaration).
- `rules_rust`, `rules_go`, `rules_apple`: cooperative-resolution-plus-reimplemented-execution exemplars.
- Gradle Develocity: what good build telemetry looks like at scale.
