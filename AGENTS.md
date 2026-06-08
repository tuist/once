# Working on Once

Conventions for humans and AI agents contributing to this repo.

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

New build graph CLI, rule, and query surfaces should be introduced
deliberately and documented in the relevant RFC or implementation plan.

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

## Toolchain

The repo pins `rust = "1.88"` in `mise.toml` and the workspace
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

## Style

- No em dashes in user-facing text: README, docs, PRs, commits, and
  release notes. Rewrite with a comma, semicolon, or sentence break.
- No roadmap-phase numbers in code or doc comments. Describe behavior,
  not milestones, since milestones rename and the comments rot.
- Default to writing no comments. Add one only when the why is
  non-obvious.

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
