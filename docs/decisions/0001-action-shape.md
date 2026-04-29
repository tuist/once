# ADR 0001: Action shape with typed inputs, typed outputs, and integration mode as a first-class field

- **Status:** Proposed
- **Date:** 2026-04-29

## Context

Fabrik's executor today knows one action kind: `RunCommand { argv, env, cwd, timeout_ms }`. That was enough for Phase 0 (cache a subprocess). It is not enough to express the work that any real language plugin needs to emit. The Phase 1 rust plugin will be the first non-toy consumer, and once any plugin emits actions, the action's wire format becomes part of the cache key. Changes after that point require either a domain-prefix bump (cache flag day) or a compatibility shim. So the shape we pick now is the shape we live with.

This is a load-bearing decision. To avoid reinventing pain that's been studied for a decade, we surveyed the most-used Bazel rule sets (`rules_rust`, `rules_go`, `rules_apple`, `rules_swift`, `rules_android`, `rules_jvm_external`, `rules_kotlin`, `aspect-build/rules_js`) and looked at where their designs converged, where they diverged, and where they have sustained pain. The full notes are in the PR discussion; this ADR records the conclusions we drew and the choices that follow.

The headline lessons:

1. **Bazel got the action primitive right.** Every mature ruleset uses the same shape: `(tool, inputs: depset<Artifact>, outputs, args, env)`. Twelve years on, nobody has invented a better one. We adopt it.
2. **Where Bazel suffers is where it left things untyped.** Stringly-typed action attributes, `genrule` outputs, opaque `DefaultInfo.files` at integration boundaries. Every one of those is a place where rule sets later grew bucketing schemes (`AppleResourceInfo`), policy strings (`maven_install` version conflicts), or per-crate `annotations` (`rules_rust` `*-sys`) to retrofit structure. Skip the intermediate stage.
3. **Bazel never surfaces what *kind* of integration is in play.** rules_rust silently mixes reimplemented (workspace), cooperative (third-party via `crate_universe`), and opaque (sys-crates via per-crate `annotations`). Users debugging cache misses or non-determinism have no protocol-level signal. We can fix this for free by tagging actions with their integration mode at the protocol level.
4. **A few problems are sustained across every ecosystem.** Two-source-of-truth between native config and the build description, build scripts escaping hermeticity, mixed-language modules in one compilation unit, opaque outputs feeding owned consumers. Our model has to make these *easier*, or we have not improved on Bazel.

## Decision

The core action primitive in `fabrik-core` becomes:

```rust
pub struct Action {
    /// What runs.
    pub invocation: Invocation,

    /// Files, env, and tool versions the action depends on. Content-
    /// addressed; folded into the action digest.
    pub inputs: Inputs,

    /// Files the action will produce. Captured into CAS by the
    /// executor on completion.
    pub outputs: Vec<DeclaredOutput>,

    /// strict / traced / loose. The executor enforces.
    pub sandbox: Sandbox,

    /// Worker support, RBE platform constraints, network access, etc.
    pub execution: ExecutionRequirements,

    /// Which plugin emitted this action and at what version. Part of
    /// the cache key namespace; never silently shared across plugin
    /// versions.
    pub plugin: PluginRef,

    /// Reimplemented | Cooperative | Opaque. First-class on the
    /// action, not an out-of-band attribute.
    pub mode: IntegrationMode,
}

pub struct Invocation {
    pub argv: Vec<String>,
    pub env: BTreeMap<String, EnvValue>,
    pub cwd: Option<WorkspacePath>,
    pub timeout_ms: Option<u64>,
}

pub enum EnvValue {
    /// A literal string.
    Literal(String),
    /// The path of an input artifact, materialized at execution time.
    /// Lets actions reference inputs without hard-coding paths.
    ArtifactPath(ArtifactRef),
}

pub struct Inputs {
    /// Workspace-relative source files. Each carries its content
    /// digest at the time the action was emitted.
    pub files: Vec<FileInput>,
    /// Outputs of upstream actions consumed by this one. Edges in the
    /// build graph.
    pub artifacts: Vec<ArtifactRef>,
    /// Tool identities and versions. Probed once per workspace per
    /// run; their fingerprints participate in the cache key.
    pub tools: Vec<ToolDecl>,
}

pub struct FileInput {
    pub path: WorkspacePath,
    pub digest: Digest,
}

pub struct ArtifactRef {
    pub producer: ActionDigest,
    pub name: String,
}

pub struct ToolDecl {
    pub name: String,        // "rustc", "swiftc", "javac", "gradle"
    pub version: String,     // result of `<tool> --version` (canonical form)
}

pub struct DeclaredOutput {
    /// Logical name within the action ("rlib", "rmeta", "obj").
    pub name: String,
    /// Where the action will write it, relative to a per-action
    /// scratch root. Captured into CAS as <kind, digest>.
    pub workspace_path: WorkspacePath,
    pub kind: ArtifactKind,
}

pub enum ArtifactKind {
    // Universal across languages.
    Source,
    Object,
    StaticLib,
    DynamicLib,
    Executable,

    // Per-language compiled artifacts.
    RustRlib,
    RustRmeta,
    SwiftModule,
    SwiftInterface,
    Jar,
    AbiJar,
    ClassFile,
    Dex,

    // Higher-level packages.
    Resource(ResourceKind),
    Bundle(BundleKind),

    /// Schema-tagged escape hatch. The producer declares a schema
    /// identifier so consumers can validate at hookup time even when
    /// Fabrik cannot introspect the contents. e.g. "android.aar.v1",
    /// "gradle.outputs.tar.v1", "xcodebuild.framework.v1".
    OpaqueBlob { schema_id: String },
}

pub enum IntegrationMode {
    /// Plugin owns the toolchain invocation: emits per-unit actions
    /// with declared inputs/outputs. rustc per crate. Maximum cache
    /// fidelity.
    Reimplemented,
    /// Plugin uses the native tool to discover the graph but executes
    /// via the substrate. cargo metadata + rustc actions. Mid-fidelity.
    Cooperative,
    /// Plugin runs the native tool whole. gradle build, vite build.
    /// Inputs and outputs are coarse but honest.
    Opaque,
}

pub enum Sandbox {
    Strict,
    Traced,
    Loose,
}
```

The action digest is `BLAKE3(domain || canonical_serialization(self))` where `domain = b"fabrik.action.v2\0"`. Bumping `v2` to `v3` is the formal escape hatch when this schema must change incompatibly.

## Why each piece

### Typed `Inputs` rather than just argv + env

The Phase 0 `RunCommand` collapses everything into argv and env. That works for a single subprocess but loses the structure every plugin needs:

- The plugin knows which files are sources and which are upstream artifacts; the executor needs that distinction to schedule and to share artifacts remotely.
- The plugin knows which env vars carry tool versions vs. which are runtime config. The cache should treat `RUSTC_VERSION=1.86.0` and `TERM=xterm` differently, and a typed `ToolDecl` makes that obvious.
- Cross-language flow becomes natural: a Rust action consumes `ArtifactRef { producer: <protoc-action>, name: "user.rs" }` without either side knowing the other's language. (Bazel learned this through `CcInfo`-as-lingua-franca, which is the same pattern formalised.)

### `OpaqueBlob { schema_id }`

This is the piece Bazel doesn't have and visibly suffers from. When `rules_apple` produces a `.framework`, when `rules_jvm_external` resolves a `.jar`, when any Gradle integration produces a build directory, Bazel falls back to `DefaultInfo.files` (an untyped depset) and consumers either trust it or write defensive checks every time. By contrast our opaque artifacts still carry a declared schema string. A future Java plugin consuming an Android AAR can refuse to hook up if it sees `schema_id = "rust.crate.tar.v1"` instead of `"android.aar.v1"`. The schema string is the minimum amount of structure needed to make opaque mode safe to compose.

### `IntegrationMode` on the action

`rules_rust` is the cautionary tale here. A user does `bazel build //my_crate`, gets a cache miss, has no idea whether they hit the workspace-rules path (deterministic, our fault), the crate_universe path (deterministic if the lockfile didn't drift), or a sys-crate `build.rs` path (probably non-deterministic). Three completely different debugging paths, no signal.

By making `IntegrationMode` a field on every action:

- The query API can answer "why did this rebuild?" with mode-specific narrative ("opaque mode against `gradle 8.7.0` cannot introspect outputs; the entire `build/` directory was reconsumed because one input changed").
- The cache statistics can break down hit rate by mode, telling operators where their fidelity is leaking.
- The CLI can warn loudly when a workspace mixes a lot of opaque actions, the way `rustc` warns about `unsafe` blocks.

The cost is one enum variant in the wire format. The benefit is a load-bearing differentiator from Bazel that costs us nothing structurally.

### Separate `PluginRef`

Bazel cache-shares results across rule-set versions by default, on the assumption that minor version bumps don't change behavior. That assumption is wrong roughly as often as plugin maintainers wish it weren't. We make plugin name+version part of the action digest namespace; cache-sharing across plugin versions is *opt-in*, declared by a plugin signalling schema compatibility with its predecessor. Loud by default, quiet when explicitly safe.

### Rejecting one provider per language

Bazel's path was `JavaInfo`, then `KtJvmInfo`, then `AndroidIdeInfo`, then `AndroidResourcesInfo`, then `StarlarkAndroidResourcesInfo`. The Android Starlark migration (rules_android #77) showed how brittle the resulting cross-provider coupling is. We deliberately do not introduce a typed provider layer in this ADR. Artifacts (typed, content-addressed) flow between actions; "providers" can be inferred later if patterns emerge. rules_apple's bucketed `AppleResourceInfo` and rules_go's three-way `GoLibrary`/`GoSource`/`GoArchive` split both *evolved*; neither was designed up front.

### Rejecting "one rule, one language"

The longest-open issues across Bazel (rules_apple #179 and #240 for Swift+ObjC mixed module, the cgo bridge gaps, Kotlin+Java `kotlinc` ordering) all stem from the same assumption: that a build node is single-language. Our `Action` is a tool invocation, not a language unit. Nothing in the schema cares whether `inputs.files` are all `.swift` or a mix of `.swift` and `.m`. We don't *solve* mixed-language modules in this ADR (that requires plugin-level coordination), but we don't *prevent* solving them either.

## Consequences

### Better

- **One executor model serves every plugin**, from reimplemented Rust to opaque Gradle. The mode field encodes the difference.
- **Cache fidelity becomes queryable.** "Why did this rebuild" answers can cite the integration mode, not just the digest.
- **Cross-language flows are first-class.** A Rust action consuming a protoc artifact is exactly the same shape as a Rust action consuming another Rust crate's `.rmeta`.
- **Opaque mode is safe to compose** because the `schema_id` survives the boundary.
- **Plugin versioning is loud by default**, eliminating one of `rules_rust`'s sustained pain points.

### Worse

- **The action wire format is bigger** (more fields, more enums) than the Phase 0 `RunCommand`. Slightly more bytes per cache key, slightly more deserialization cost on every cache check. We're betting the structural wins outweigh the few-hundred-bytes-per-action overhead. If it ever matters, the canonical encoding is replaceable behind the digest domain.
- **Plugins have to declare more.** A toy plugin that just wants to spawn a subprocess now has to fill in `Inputs`, `outputs`, `mode`. We mitigate by providing a `RunCommand`-shaped helper in `fabrik-core` for the trivial case (current CLI `fabrik run` will use it).
- **The schema is now public surface.** Once plugins exist, schema changes need a domain bump. We accept this cost; it's the price of the contract.
- **`OpaqueBlob` schema strings are an ad-hoc namespace.** No central registry, no validation beyond string equality. We considered formal schema registration and rejected it as over-engineering at this stage. If `schema_id` collisions become a real problem we'll add a registry; until then, conventions documented per plugin are enough.

### Neutral

- **Phase 0's `RunCommand` becomes a special case of the new shape.** The CLI `fabrik run` keeps working: it constructs an action with empty `inputs.files`/`inputs.artifacts`, no declared outputs, `Sandbox::Loose`, `IntegrationMode::Opaque`, and the existing argv/env/cwd/timeout. The migration is mechanical.

## Alternatives considered

### A. Closed enum of action kinds (`Compile`, `Link`, `Test`, ...)

The most "obviously typed" option: model action *kinds* explicitly. Rejected for two reasons:

- Every language adds new kinds (Swift module-merge, Java ABI-jar, Android dex-merger). The enum grows without bound and every change is a breaking schema bump.
- The executor would have to know how to execute each kind. That puts language-specific logic in the core, which is exactly what plugins are supposed to encapsulate.

The action shape we adopted is *kind-agnostic*. The executor only knows "spawn this tool with these inputs and capture these outputs." Plugins encapsulate the language semantics.

### B. Free-form key/value payload (`Action { kind: String, payload: Value }`)

The other extreme: completely opaque payload, plugins put whatever they want in. Rejected because:

- The cache key would have to hash a `serde_json::Value`, which has stability problems (number types, escape encoding, struct field ordering).
- No type safety at the executor boundary; misuses surface as runtime errors instead of compile errors.
- Cross-plugin composition becomes guesswork. The Java plugin can't trust what shape an artifact from the Rust plugin has.

The typed `ArtifactKind` enum gives most of the flexibility (`OpaqueBlob` is the escape hatch) without giving up the validation.

### C. Borrow Bazel's REAPI verbatim

REAPI is the protocol behind Bazel Remote Execution and we will be wire-compatible with it at the substrate layer. But REAPI's `Action` message is shaped for *remote execution* specifically: it elides the plugin/mode/sandbox concerns we care about, and its `command_digest`/`input_root_digest` indirection is overhead we don't need locally. We lower our `Action` to REAPI when we send it over the wire, but we don't adopt REAPI as our internal shape. Same architectural pattern Bazel itself uses.

### D. Defer this decision

Tempting: ship the rust plugin against the existing `RunCommand`, see what hurts, then redesign. Rejected because:

- The action's wire format is the cache key. Once the rust plugin emits one action, every subsequent change is a cache flag day.
- The plugin contract surface is what makes Fabrik distinct from a glorified subprocess cache. Designing against a real consumer and locking in the contract before another plugin lands is the right sequencing.

## Out of scope (deliberately)

This ADR does *not* settle:

- **Provider system.** No typed-info-flowing-between-rules layer in this ADR. Artifacts flow; everything else is plugin-local. Future ADRs will introduce capability-keyed providers (`Compiled`, `Linkable`, `Bundleable`, `Runnable`) once the rust + (likely) C plugins surface real composition needs.
- **Mixed-language compilation units.** Schema doesn't *prevent* them but doesn't *solve* them. A future ADR will tackle Swift+ObjC and Kotlin+Java once the single-language case is solid.
- **Persistent-worker protocol.** `ExecutionRequirements` has a hook for it but the protocol shape is its own ADR.
- **Cross-plugin cache sharing semantics.** `PluginRef` puts plugin name+version in the cache namespace; the rules for *when* two versions can share a slot are deferred until we have two plugins at different versions.
- **REAPI lowering.** The mapping from this `Action` to REAPI's `Action` message is deferred until the substrate grows the remote-cache tier (Phase 7 in the roadmap).
- **Opaque-blob schema registry.** Strings, conventions per plugin, no central enforcement. Revisit when collisions matter.

## References

### Bazel rule sets surveyed (informing every decision in §"Why each piece")

- **rules_rust**: [crate_universe docs](https://bazelbuild.github.io/rules_rust/crate_universe.html), which describes cooperative resolution via `cargo metadata` plus a lockfile-of-BUILD-files. This is the two-source-of-truth pain that motivated our "native file is the only source of truth" position. Tweag's [Building a Rust workspace with Bazel](https://www.tweag.io/blog/2023-07-27-building-rust-workspace-with-bazel/) catalogs the `*-sys` crate annotation pain that motivated our `IntegrationMode::Opaque` being explicit.
- **rules_go**: [providers.rst](https://github.com/bazelbuild/rules_go/blob/master/go/providers.rst) for the `GoLibrary`/`GoSource`/`GoArchive` three-layer split and the "all providers must hold only immutable data" rule that informed our decision to make `Action` immutable and depset-friendly.
- **rules_apple / rules_swift**: [resources.bzl bucketing](https://github.com/bazelbuild/rules_apple/blob/master/apple/internal/resources.bzl), [SwiftInfo](https://github.com/bazelbuild/rules_swift/blob/master/doc/providers.md), and the years-open mixed-module issues [#179](https://github.com/bazelbuild/rules_apple/issues/179) and [#240](https://github.com/bazelbuild/rules_apple/issues/240). These motivated rejecting one-rule-one-language and adopting the typed `ArtifactKind` enum (vs. AppleResourceInfo's after-the-fact buckets).
- **rules_android**: [Bazel #5354](https://github.com/bazelbuild/bazel/issues/5354) Starlark migration, [rules_android #77](https://github.com/bazelbuild/rules_android/issues/77) `aar_import` provider gaps. Motivates rejecting one-provider-per-feature.
- **rules_jvm_external**: [README](https://github.com/bazelbuild/rules_jvm_external). Coursier-resolves-into-`maven_install.json` is the model for our cooperative-mode lockfile pattern.
- **aspect-build/rules_js**: [rewrite design doc](https://hackmd.io/@aspect/rules_js) which states the explicit "fast and deterministic vs 100% compatibility" trade-off the rewrite chose. Confirms our position that opaque mode for bundlers is the right answer, not heroics.
- Gradle interop landscape: [Grazel](https://github.com/grab/grazel), [sgammon/rules_gradle](https://github.com/sgammon/rules_gradle), [Gradle's Build with Bazel](https://blog.gradle.org/gradle-vs-bazel-jvm). Three serious attempts, none works at production scale, which informs our position that Gradle is `Opaque` or nothing.

### Internal

- [docs/design.md](../design.md) §2 (Architecture), §4 (Plugin model), §4.1 (the three integration modes).
- [docs/roadmap.md](../roadmap.md) Phase 1. This ADR resolves the "decide between (a) keeping `Action` closed and lowering plugin actions to `RunCommand` primitives, or (b) opening it" question called out there.
- [crates/fabrik-core/src/lib.rs](../../crates/fabrik-core/src/lib.rs) for the current `Action` enum and `ACTION_DIGEST_DOMAIN` that this ADR proposes to grow.

## Open questions

These remain open after this ADR; each will need its own ADR or design discussion:

1. **Canonical encoding of `Action` for the digest.** Currently `serde_json` (versioned by the domain prefix). When we add binary fields (artifact contents), JSON gets awkward; CBOR with deterministic encoding is the likely successor. Defer until a binary field actually appears.
2. **`ToolDecl::version` canonicalization.** `cargo --version` returns `cargo 1.86.0 (e0b80b734 2025-03-12)`; we want a deterministic substring. Per-tool normalization helper, deferred.
3. **`ExecutionRequirements` shape.** Listed in the schema but unspecified. The persistent-worker protocol, RBE platform constraints, and network-access policy each deserve their own design.
4. **Whether `mode` should be derived or declared.** Currently the plugin declares it. Could the executor infer it from the input/output structure? Probably not safely (a cooperative-mode action that happens to declare every input still belongs in the cooperative bucket), but worth revisiting once we have multiple plugins.
