---
name: once-rust-review
description: Project-specific PR-review rules for the tuist/once Rust workspace. Focuses on script caching, build graph work, runtime execution, `once.toml` placement, toolchain pinning, shellspec coverage, and crate structure.
---

# Once Rust Review

This skill is intentionally narrow. **Generic Rust style, formatting,
naming, and most lint hygiene are already covered by `rustfmt` and
`clippy` in CI, so do not flag those.** Focus on the repo-specific
rules below.

For each finding, cite `path:line` and quote the relevant snippet.

---

## 1. Build graph work follows the RFC and keeps scripts first-class

Once starts with cacheable and remotely executable scripts plus the runtime
API, and is expanding toward typed build graph capabilities. Scripts remain the
migration ramp into the graph. Build graph work should follow
`rfcs/0001-build-graph.md`, keep the agent-facing graph model typed,
queryable, and structurally editable, and preserve the existing script
workflow.

### Flag

- **Build graph CLI, rule, or query surfaces that are not tied to the RFC
  model, lack a clear migration path from scripts, or make scripts
  second-class.** **Severity: high.**
- **Docs or CLI help that position Once as Buck-compatible, Bazel-compatible,
  or as a drop-in replacement for Buck, Bazel, or Cargo.** **Severity:
  medium.**
- **Manifest parsing that accepts non-script target rules without a
  clear migration path and tests.** **Severity: high.**
- **Build graph changes that bypass Once's action cache, CAS, remote execution,
  or runtime API instead of using them as the execution substrate.**
  **Severity: medium.**

### Do not flag

- Script or runtime features that make individual commands cacheable,
  observable, or remotely executable.
- Build graph commands, providers, capabilities, queries, Starlark rule
  metadata, or Apple target modeling that align with `rfcs/0001-build-graph.md`
  and include focused Rust tests or ShellSpec coverage.

---

## 2. Project manifests live in `once.toml`

Per-project configuration is `once.toml`. Runtime cache state and output
directories are not source-controlled manifest locations.

### Flag

- **A new checked-in manifest under runtime cache or output directories,
  or code that treats generated cache contents as source manifests.**
  **Severity: high.**
- **Docs or tests that instruct contributors to edit cache directories
  as canonical project config.** **Severity: medium.**
- **A change that reintroduces `fabrik.toml` as a supported project
  manifest.** **Severity: high.**

### Do not flag

- Tests that intentionally assert cache and output directories are
  skipped during workspace discovery.

---

## 3. Toolchain pinning and command invocation must stay aligned

The repo deliberately pins Rust in both `mise.toml` and the workspace
`Cargo.toml`, and contributor-facing commands should use `mise exec --`.

### Flag

- **A diff that changes `mise.toml` `rust = "..."`
  without the matching workspace `rust-version` change in `Cargo.toml`,
  or vice versa.** **Severity: medium.**
- **A toolchain bump without matching spec updates or without any
  explanation in the diff.** **Severity: medium.**
- **Contributor docs, scripts, or tests that invoke `cargo` directly for
  this repo instead of `mise exec -- cargo ...`.** **Severity: medium.**

### Do not flag

- Internal implementation code that shells out to Cargo as product
  behavior rather than contributor workflow documentation.

---

## 4. CLI contract changes need ShellSpec coverage

`spec/*.sh` is the end-to-end contract for the CLI.

### Flag

- **A change under `crates/once-cli/src/` that alters user-visible
  command behavior, help text, stdout/stderr, exit status, or declared
  cache/runtime behavior without matching ShellSpec coverage.**
  **Severity: medium.**
- **A new CLI subcommand or error message path without an end-to-end spec
  exercising it.** **Severity: medium.**

### Do not flag

- Pure refactors that do not change observable CLI behavior.

---

## 5. Runtime and cache behavior changes need focused tests nearby

Most semantic behavior in this repo is verifiable with in-process Rust
tests next to the module that changed.

### Flag

- **A semantic change in `crates/once-core`, `once-frontend`,
  `once-cli`, or `once-cas` without focused unit tests near the changed
  code or a strong end-to-end spec covering the same behavior.**
  **Severity: medium.**
- **Changes to digest stability, manifest parsing, cache keys, runtime
  descriptors, remote execution selection, or toolchain resolution that
  land without tests.** **Severity: medium.**

### Do not flag

- Test-only refactors or comment-only changes.

---

## 6. Keep `lib.rs` and `main.rs` as dispatch tables

This repo prefers crate roots that read like a table of contents:
`mod` declarations, re-exports, and light dispatch, not mixed
implementation.

### Flag

- **A new or heavily expanded `lib.rs` / `main.rs` that mixes unrelated
  logic or grows into a monolith instead of splitting modules.**
  **Severity: low.**

### Do not flag

- Small crate roots that remain mostly declarations and re-exports.

---

## 7. No em dashes in user-facing text

README copy, CLI help, error messages, commit text, and docs should not
use em dashes in this repo.

### Flag

- **New user-facing strings or documentation that introduce an em dash
  character.** **Severity: low.**

---

## Out of scope

- Generic naming, formatting, or lint nits already enforced by
  `rustfmt`/`clippy`
- Suggestions to add comments where the code is already clear
- Suggestions to move tests away from the module they exercise
