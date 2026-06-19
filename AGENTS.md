# Working on Once

Conventions for humans and AI agents contributing to this repo.

## GitHub

- Use Conventional Commit style for pull request titles, such as
  `fix(docs): update deployment route`.
- Do not prefix pull request titles with `[codex]`.

## Module Layout

**Avoid monolith Rust files.** When a `lib.rs` or `main.rs` grows past
roughly 200 lines or starts mixing unrelated concerns, split it into a
module per responsibility. The top-level file should read as a table of
contents: re-exports, `mod` declarations, and dispatch.

Tests live next to the code they exercise: `#[cfg(test)] mod tests { ... }`
inside each module file. Cross-module integration tests go under
`crates/<crate>/tests/`.

## Manifest Files

- Per-package manifests are named `once.toml`.
- The `.once/` directory at the workspace root is cache and runtime
  state, not a manifest. It is gitignored.
- Root `once.toml` configures workspace-level settings such as cache
  providers. Package `once.toml` files may grow build graph declarations
  as Once expands beyond script-only workflows.
- Scripts declare their execution metadata with `# once` headers in the
  script file.

## Scope

Once starts with cacheable and remotely executable scripts plus the
runtime API, and is expanding toward build-system capabilities. Scripts
remain the migration ramp into the Once build graph. Build graph work
should follow RFCs and keep the agent-facing graph model typed,
queryable, and structurally editable.

Keep the current CLI surface centered on:

- `once exec` for literal commands and annotated script files
- `once cache` for CAS and action-cache primitives
- `once runtime` for JSON-RPC runtime sessions
- `once auth` and `once toolchain` for supporting infrastructure

New build graph CLI, target kind, and query surfaces should be introduced
deliberately and documented in the relevant RFC or implementation plan.

Generic surfaces must stay ecosystem-neutral. CLI commands, Rust APIs,
MCP tools, and shared graph/query records should not hardcode examples,
field names, or behavior around one toolchain such as Apple, Cargo,
npm, SwiftPM, or crates.io. Put ecosystem-specific behavior behind a
resolver or Starlark target kind implementation, or behind a dedicated
toolchain guide/reference page so future ecosystems can plug into the
same shape instead of requiring parallel CLI or MCP surfaces.

## Graph Modules

Once exposes a doc-less surface for coding agents: an agent should be
able to discover what target kinds exist, pull a runnable starter, validate a
draft, and commit an edit using MCP tool calls alone, without reading
prose docs. When adding support for a new toolchain (Android, JVM,
Rust, etc.), mirror the shape already established for the Apple target kinds
rather than inventing a parallel surface.

Rust code must stay toolchain-agnostic. Do not add Rust branches that
recognize Apple, Android, JVM, Rust, or any other build system by name.
Build system behavior belongs in target kinds. The Rust side should provide
generic primitives, typed graph plumbing, validation surfaces, and
execution policy that target kinds can compose to express their needs.

Starlark target kind contract changes must update the public Starlark modules
reference in the same change. This includes new globals, changed `ctx`
fields, action declaration semantics, provider expectations, schema
helpers, loading behavior, or project module authoring guidance. Shared
Starlark helpers should live in the common prelude instead of being
copied into each toolchain file. The Starlark prelude index owns the
built-in module source order, so adding or removing a target kind family should
not require Rust executor changes.

Every new toolchain target kind should preserve these invariants:

- The target kind is discoverable through `once_list_target_kinds` and its full
  contract is fetchable through `once_query_schema`.
- The target kind ships at least one runnable starter example as a real
  directory owned by the module package (manifest plus sources). The
  exported Starlark target kind symbol declares it through
  `example(slug, name = ..., use_when = ..., path = ...)`; `path`
  defaults to `examples/<slug>`, and inline TOML strings are not
  allowed. Example paths are relative to the module package and must stay
  inside it so target kinds can be published through a registry as a portable
  unit.
- Every example loads under the examples integration test
  (`crates/once-frontend/tests/examples.rs`) without emitting any
  diagnostics. If the target kind has an `impl`, the example must build.
- User-visible failures surface through the structured `Diagnostic`
  shape (`code`, `target`, `attribute`, `repairs`). Validation lives
  in `target_validator`; the editor in `manifest_editor` reuses the
  same shape so retries are single-shot for the agent.
- Every MCP tool has a matching `once query` or `once edit` CLI
  subcommand so a human can reproduce what an agent does from the
  terminal.

The built-in Apple target kinds and their portable starter examples are the
reference implementation. Treat their module-owned examples, schema
metadata, validation, and MCP/CLI discovery shape as the template when
wiring a new toolchain.

## SDK API And Docs

The `once` crate root and `crates/once/swift/Once.swift` are public SDK
surfaces. Keep them centered on cache access unless an explicit product
decision expands them. Do not expose script execution, runtime sessions,
frontend parsing, or provider internals through the SDK by accident.

When changing the Rust or Swift SDK API, update `docs/guide/sdk/` in the
same change. Treat method names, return types, default cache behavior,
memory ownership, and async behavior as compatibility-sensitive. Avoid
regressions in the public API and call out deliberate breaking changes in
the pull request description.

## Tests

- Rust unit tests cover in-process behavior such as digest stability,
  parser errors, cache key partitioning, and runtime query logic.
- Shellspec (`spec/*.sh`) covers the CLI's external contract end to
  end. Run `mise exec -- shellspec` after a release build.

## Internal Logging

Every `once` CLI invocation creates a UUIDv7 session log under the
platform log directory. On Linux this follows XDG state directory
conventions; on macOS logs land under `~/Library/Logs/Once` so they are
visible from Console.app.

Use `tracing` for internal execution logs instead of printing debug
information to stdout or stderr. Log enough structured context to
reconstruct execution: command surface, target ids, action digests,
cache hit or miss decisions, remote provider choices, retry attempts,
durations, and failure causes. Do not log secrets, auth tokens, full
environment dumps, or command arguments that may contain credentials.
Prefer fields over string interpolation so logs stay queryable.

## Toolchain

The repo pins `rust = "1.96.0"` in `mise.toml` and the workspace
`rust-version`. Bumping the toolchain affects the user-facing MSRV;
do it deliberately, not as a side effect of adding a dependency. The
Windows CI job reads the workspace `rust-version` so it stays aligned
with the same pin.

## Native Dependencies

Linux builds need `libcap-ng-dev` because the embedded Microsandbox
provider links through native KVM support. Install it before running
workspace builds locally on Linux:

```sh
sudo apt-get update && sudo apt-get install -y libcap-ng-dev
```

## Android Toolchain

Run the Android setup task before building or testing Android target kinds
locally:

```sh
mise install
mise run android:install-sdk
```

The task accepts Android SDK licenses, installs the SDK packages used by the
bundled Android examples, and creates `~/.android/debug.keystore` when it is
missing. Run it after a fresh checkout or after changing Android SDK versions
in `mise.toml`.

## Style

- No em dashes in user-facing text: README, docs, PRs, commits, and
  release notes. Rewrite with a comma, semicolon, or sentence break.
- No roadmap-phase numbers in code or doc comments. Describe behavior,
  not milestones, since milestones rename and the comments rot.
- No source code paths in user-facing docs. The website and release
  notes describe what Once does and how to use it, never where the
  code lives. Source paths rot under refactors, mean nothing to a
  reader who isn't editing the repo, and leak implementation detail
  through the public surface. Describe behavior, link to the
  reference, or quote `once.toml` shapes instead.
- Default to writing no comments. Add one only when the why is
  non-obvious.

## Concurrency

Parallelize as much as possible. When work units are independent (graph
target builds, action executions, network fetches, file reads, test
runs) drive them concurrently with tasks, joins, or `try_join` rather
than serialising them. Sequential code should be a deliberate choice
for data dependencies or ordering constraints, not the default.

## Running Things

```sh
mise install
mise exec -- cargo test --workspace
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --all -- --check
mise exec -- cargo build --release
mise exec -- shellspec

mise exec -- target/release/once exec -- cargo check --workspace
mise exec -- target/release/once exec -- /bin/sh -c 'printf hello'
```

`mise exec --` is required because the project's toolchain is mise-managed;
calling `cargo` directly will miss the pinned rustc.
