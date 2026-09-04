---
prev: false
next: false
---

# Bazel

Once reads a [Bazel](https://bazel.build) workspace through `bazel query`,
exposes every rule as a Once target, and runs each target's actions
itself. Bazel handles analysis (target loading, toolchain resolution,
external repository download via `bazel fetch`, the action graph via
`bazel aquery`); Once handles execution, from a shadow execution root
under `<workspace>/.once/bazel-shadow/<target>/`. `bazel build` is only
invoked as a fallback when a target's action graph contains a
Bazel-internal action that has no argv in `aquery` output (`Symlink`,
`FileWrite`, `RunfilesTree`, `SymlinkTree`, `RepoMappingManifest`). Each
such mnemonic Once learns to run natively shrinks the fallback set.

## Try an Existing Workspace

Once recognizes `MODULE.bazel`, `WORKSPACE.bazel`, and `WORKSPACE`
automatically. This works without `once.toml`:

```sh
once query targets
once build --ui
once test
```

The generated `bazel_workspace` seed runs
`bazel query 'kind("rule", //...)' --output=label_kind`, materializes every
rule as a `bazel_target`, and forwards the label and rule class on
`bazel_label` and `bazel_rule_kind`. Discovery skips generated
`bazel-bin`, `bazel-out`, and `bazel-testlogs` directories.

When discovery finds one Bazel workspace, `once build --ui` builds its
non-test roots and opens the Runs interface. `once test` runs the test rules
reported by that workspace. Use `once test --ui` to follow scheduling and
results in the same interface. Discovery does not write an Once manifest.

## Ignore The Once Cache

Bazel walks every directory the workspace contains unless the repository
lists it in
[`.bazelignore`](https://bazel.build/reference/command-line-reference#flag--noenable_bzlmod).
Once writes its cache and outputs under `.once/`. Add a single line to
`.bazelignore` next to `MODULE.bazel`:

```
.once
```

Without that entry, checked-in `BUILD.bazel` files under `.once/out/` from
other native integrations can be picked up by `bazel query` and fail graph
loading with a missing rule loader.

## Keep Bazel Commands

Once can sit behind the `bazel` command for one native workspace. Add this
mise wrapper, then run `mise reshim` and activate mise in the shell that
starts the build:

```toml
[wrappers.bazel]
command = "once"
args = ["bazel", "--"]
```

Once routes labeled build and test forms into the native graph:

```sh
bazel build //src:kura
bazel test //src:kura_lib_test
```

The wrapper accepts one fully qualified label per invocation. Every other
request, including wildcards (`//...`), extra flags (`--config=debug`),
queries, and `bazel run`, invokes the system `bazel` executable with its
original arguments and exit status. This preserves Bazel behavior until the
request has an exact Once equivalent.

For troubleshooting without mise, call the compatibility surface directly and
put the separator before the Bazel arguments:

```sh
once bazel -- --version
once bazel -- query "kind(rule, //:*)"
```

Set `ONCE_BAZEL_PATH` to select a specific `bazel` executable when the
wrapper is active. The default resolution walks `PATH` and honors
[`bazelisk`](https://github.com/bazelbuild/bazelisk) shims.

See [`bazel_workspace`](/reference/prelude/bazel_workspace) and
[`bazel_target`](/reference/prelude/bazel_target) for their complete
contracts.

## Prerequisites

Install Bazel (or Bazelisk, which selects the version pinned by the
workspace) and verify it resolves in the shell that runs Once:

```sh
bazel --version
```

The graph loader shells out to `bazel query` during resolution. A Bazel
workspace that cannot load through the raw `bazel` command cannot load
through Once either. Diagnose loader errors with the same `bazel query`
invocation from the workspace root before revisiting Once.
