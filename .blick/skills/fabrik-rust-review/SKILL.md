---
name: fabrik-rust-review
description: Project-specific PR-review rules for the tuist/fabrik Rust workspace. Focuses on the things only this repo knows: the adopted root build path, `fabrik.toml` placement, workspace-local `.fabrik/out` semantics, toolchain pinning, shellspec coverage, and keeping crate roots small.
---

# Fabrik Rust Review

This skill is intentionally narrow. **Generic Rust style, formatting,
naming, and most lint hygiene are already covered by `rustfmt` and
`clippy` in CI, so do not flag those.** Focus on the repo-specific
rules below.

For each finding, cite `path:line` and quote the relevant snippet.

---

## 1. The repo root stays on the adopted `cargo_binary` path

The workspace root currently ships `fabrik` through a root
`[[cargo.binary]]` target in `fabrik.toml`. That is the production path
until granular third-party build-script and feature support is complete.

### Flag

- **A diff that changes the repo-root `fabrik.toml` away from
  `[[cargo.binary]]` for `name = "fabrik"` / `cargo_package =
  "fabrik-cli"` without also showing that the missing granular
  third-party support is implemented.** This is a correctness and
  self-hosting risk. **Severity: high.**
- **Docs or CLI help that claim the repo self-host now builds through
  the granular Rust path by default.** **Severity: medium.**

### Do not flag

- Example workspaces under `examples/` that intentionally demonstrate
  granular targets.

---

## 2. Build manifests live in `fabrik.toml`, not under `.fabrik/`

The `.fabrik/` directory is the workspace cache and output area. It is
not a source-controlled manifest location.

### Flag

- **A new checked-in `fabrik.toml` under `.fabrik/` or code that treats
  `.fabrik/**/fabrik.toml` as a real package manifest.** This confuses
  cache state with source state. **Severity: high.**
- **Docs or tests that instruct contributors to edit `.fabrik/` as if it
  were the canonical project config.** **Severity: medium.**

### Do not flag

- Tests that intentionally assert `.fabrik/` is skipped during workspace
  discovery.

---

## 3. Build outputs stay under workspace-local `.fabrik/out`

This repo intentionally keeps build outputs under
`<workspace>/.fabrik/out/...` rather than moving them into an XDG cache
or a global state directory.

### Flag

- **Code that relocates user-visible build artifacts away from
  `.fabrik/out/...` without an explicit compatibility plan.**
  **Severity: high.**
- **CLI help, tests, or docs that drift away from the `.fabrik/out/...`
  contract after artifact-path changes.** **Severity: medium.**

### Do not flag

- CAS storage and other internal global-cache metadata that do not
  change the user-visible output location.

---

## 4. Toolchain pinning and command invocation must stay aligned

The repo deliberately pins Rust in both `mise.toml` and the workspace
`Cargo.toml`, and contributor-facing commands should use `mise exec --`.

### Flag

- **A diff that changes `mise.toml` `rust = "..."`
  without the matching workspace `rust-version` change in `Cargo.toml`,
  or vice versa.** **Severity: medium.**
- **A toolchain bump without the matching spec updates or without any
  explanation in the diff.** **Severity: medium.**
- **Contributor docs, scripts, or tests that invoke `cargo` directly for
  this repo instead of `mise exec -- cargo ...`.** **Severity: medium.**

### Do not flag

- Internal implementation code that shells out to Cargo as product
  behavior rather than contributor workflow documentation.

---

## 5. CLI contract changes need ShellSpec coverage

`spec/*.sh` is the end-to-end contract for the CLI.

### Flag

- **A change under `crates/fabrik-cli/src/` that alters user-visible
  command behavior, help text, stdout/stderr, exit status, or declared
  artifact paths without matching ShellSpec coverage.**
  **Severity: medium.**
- **A new CLI subcommand or error message path without an end-to-end spec
  exercising it.** **Severity: medium.**

### Do not flag

- Pure refactors that do not change observable CLI behavior.

---

## 6. Planner/compiler behavior changes need focused tests nearby

Most semantic behavior in this repo is verifiable with in-process Rust
tests next to the module that changed.

### Flag

- **A semantic change in `crates/fabrik-core`, `fabrik-frontend`,
  `fabrik-rust`, `fabrik-elixir`, `fabrik-apple`, or `fabrik-cas`
  without focused unit tests near the changed code or a strong
  end-to-end spec covering the same behavior.** **Severity: medium.**
- **Changes to digest stability, manifest parsing, target planning,
  output-path generation, cache keys, or toolchain resolution that land
  without tests.** **Severity: medium.**

### Do not flag

- Test-only refactors or comment-only changes.

---

## 7. Keep `lib.rs` and `main.rs` as dispatch tables

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

## 8. No em dashes in user-facing text

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
