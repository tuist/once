---
title: Cargo dependencies without a preparation step
date: 2026-10-13
---

Hand-authored Rust targets can now use `cargo_dependencies` without first running
`cargo vendor`. Cargo fetches missing locked sources into its local cache, and
Once snapshots each package through a cacheable action. Crates remain separate
graph targets for parallel compilation and independent cache reuse.

Omit `vendor_dir` for this default workflow. If you already manage vendored
sources, set `vendor_dir` explicitly. The former implicit
`third_party/rust/vendor` default no longer applies.

Checked Cargo metadata snapshots require an explicit `vendor_dir`, with an
actionable repair when it is missing. The dependency target also requires an
existing `Cargo.lock` declared in `resolver_inputs` or `srcs`, and refuses stale
lockfiles rather than updating them during graph loading. Native
`cargo_workspace` discovery retains its existing ability to create a missing
lockfile.
