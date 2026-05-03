# Working on Fabrik

Conventions for humans and AI agents contributing to this repo.

## Module layout

**Avoid monolith Rust files.** When a `lib.rs` or `main.rs` grows past
roughly 200 lines or starts mixing unrelated concerns, split it into a
module per responsibility. The top-level file should read as a table of
contents (re-exports + `mod` declarations + dispatch), not as the
implementation itself.

A working pattern, used by `fabrik-frontend` and `fabrik-cli`:

```
crates/<crate>/src/
  lib.rs            # re-exports + module declarations
  <topic>.rs        # one focused responsibility per file
  commands/         # for the CLI: one verb per file
    mod.rs
    <verb>.rs
```

Tests live next to the code they exercise: `#[cfg(test)] mod tests { … }`
inside each module file. Cross-module integration tests go under
`crates/<crate>/tests/`.

## Build files

- Per-package build files are named `fabrik.star` (one per directory).
- Plugin and SDK modules use `*.star` (e.g. `//build/rust.star`).
- The `.fabrik/` directory at the workspace root is the **cache**, not
  a build file. It is gitignored.

## Tests

- Rust unit tests cover anything an in-process assertion can verify
  (digest stability, parser errors, cache key partitioning, etc.).
- Shellspec (`spec/*.sh`) covers the CLI's external contract end to
  end. Run `mise exec -- shellspec` after a release build.

## Toolchain

The repo pins `rust = "1.86"` in `mise.toml` and the workspace
`rust-version`. Bumping the toolchain affects the Windows CI job and
the user-facing MSRV; do it deliberately, not as a side effect of
adding a dependency.

## Style

- No em dashes in user-facing text (README, docs, PRs, commits,
  release notes). Rewrite with a comma, semicolon, or sentence break.
- No roadmap-phase numbers in code or doc comments. Describe behavior,
  not milestones, since milestones rename and the comments rot.
- Default to writing no comments. Add one only when the *why* is
  non-obvious (a hidden constraint, a workaround, a subtle invariant).

## Running things

```sh
mise install                           # install pinned rust + shellspec
mise exec -- cargo test --workspace    # all unit + integration tests
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --all -- --check
mise exec -- cargo build --release     # produces target/release/fabrik
mise exec -- shellspec                 # end-to-end CLI specs
```

`mise exec --` is required because the project's toolchain is mise-managed;
calling `cargo` directly will miss the pinned rustc.
