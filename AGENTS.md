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

- Per-package build files are named `fabrik.toml` (one per directory).
- The `.fabrik/` directory at the workspace root is the **cache**, not
  a build file. It is gitignored.
- `fabrik deps sync` writes generated dependency graph artifacts under
  `.fabrik/deps/` and generated external dependency packages under
  `.fabrik/external/`.

## Granular vs adopted Rust targets

Two paths into the build:

- **Granular** (`rust_binary`, `rust_library`, `rust_test`,
  `rust_proc_macro`): one rustc invocation per crate, with explicit
  `--extern` wiring, content-addressed caching, and CAS-restorable
  outputs. Drive this path with `fabrik build <label>`. A one-line
  edit in a leaf crate invalidates only that crate and its reverse
  deps. Cache hits restore artifacts from the CAS, which is the
  property remote execution needs. Today this path requires
  hand-written `fabrik.toml` files (or generator output) and does not
  yet thread build-script outputs into dependent rustc invocations.

- **Adopted** (`cargo_binary`): wraps `cargo build` as a single
  cached action. Use this when you need to build code whose third-party
  dependency graph contains build scripts or otherwise outpaces what
  the granular path supports. Cache granularity matches Cargo's, not
  Fabrik's, so any input change re-runs the whole `cargo build`. Drive
  this path with `fabrik run <label>`.

The Fabrik repo itself ships with `cargo_binary` at the root and is
expected to keep that as the production path until the granular
third-party graph (build scripts + per-crate features) is feature
complete.

## Third-party deps: `fabrik deps sync`

`fabrik deps sync` reads native lockfiles and emits dependency graph
metadata under `.fabrik/deps/`. Rust sync also emits
`.fabrik/external/<graph>/fabrik.toml` containing one granular Rust
declaration per crates.io dep. Pure-rust libraries and proc-macros
without build scripts are emitted live; crates with a `build.rs` are
commented out with a stub so the limitation is visible. The generator
does not copy sources today, so declarations keep empty `srcs`
placeholders until source copying exists.

Generated `.fabrik/external/<graph>/fabrik.toml` files carry a
`# fabrik:generated-external-format=<n>` stamp on the first line. The
workspace loader refuses a stamp it does not recognize so a stale
generated tree fails loudly instead of feeding a wrong graph into a
build; a missing stamp stays loadable so hand-authored external
packages keep working. Bump `GENERATED_EXTERNAL_FORMAT_VERSION` in
`fabrik-frontend` whenever the generated shape changes.

Build script support and per-target feature resolution are the open
items needed for this verb to drive a full self-host of Fabrik via
the granular pipeline.

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

# Granular per-crate build (writes outputs under .fabrik/out/):
mise exec -- target/release/fabrik build examples/rust/granular/basic-app/hello

# Generate external dependency metadata from native lockfiles:
mise exec -- target/release/fabrik deps sync
```

`mise exec --` is required because the project's toolchain is mise-managed;
calling `cargo` directly will miss the pinned rustc.
