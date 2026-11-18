---
prev: false
next: false
---

# Bazel

Struo reads a [Bazel](https://bazel.build) workspace through `bazel query`,
exposes every rule as a Struo target, and runs each target's actions
itself. Bazel handles analysis (target loading, toolchain resolution,
external repository download via `bazel fetch`, the action graph via
`bazel aquery`); Struo handles execution, from a shadow execution root
under `<workspace>/.struo/bazel-shadow/<target>/`. Unsupported internal
actions fall back to the corresponding native Bazel command. `TestRunner`
also delegates because its test environment and runfiles mapping are not
fully described by `aquery`. Delegated commands bypass the Struo action
cache; Bazel owns their build and test cache reuse.

## Try an Existing Workspace

Struo recognizes `MODULE.bazel`, `WORKSPACE.bazel`, and `WORKSPACE`
automatically. This works without `struo.toml`:

```sh
struo query targets
struo build
struo test
```

The generated `bazel_workspace` seed runs
`bazel query 'kind("rule", //...)' --output=label_kind`, materializes every
rule as a `bazel_target`, and forwards the label and rule class on
`bazel_label` and `bazel_rule_kind`. Discovery skips generated
`bazel-bin`, `bazel-out`, and `bazel-testlogs` directories.

When discovery finds one Bazel workspace, `struo build` builds its non-test
roots. `struo test` runs the test rules reported by that workspace. Discovery
does not write an Struo manifest.

## Ignore The Struo Cache

Bazel walks every directory the workspace contains unless the repository
lists it in
[`.bazelignore`](https://bazel.build/reference/command-line-reference#flag--noenable_bzlmod).
Struo writes its cache and outputs under `.struo/`. Add a single line to
`.bazelignore` next to `MODULE.bazel`:

```
.struo
```

Without that entry, checked-in `BUILD.bazel` files under `.struo/out/` from
other native integrations can be picked up by `bazel query` and fail graph
loading with a missing rule loader.

## Keep Bazel Commands

Struo can sit behind the `bazel` command for one native workspace. Add this
mise wrapper, then run `mise reshim` and activate mise in the shell that
starts the build:

```toml
[wrappers.bazel]
command = "struo"
args = ["bazel", "--"]
```

Struo routes labeled build and test forms into the native graph:

```sh
bazel build //src:kura
bazel test //src:kura_lib_test
```

The wrapper accepts one fully qualified label per invocation. Every other
request, including wildcards (`//...`), extra flags (`--config=debug`),
queries, and `bazel run`, invokes the system `bazel` executable with its
original arguments and exit status. This preserves Bazel behavior until the
request has an exact Struo equivalent.

For troubleshooting without mise, call the compatibility surface directly and
put the separator before the Bazel arguments:

```sh
struo bazel -- --version
struo bazel -- query "kind(rule, //:*)"
```

Set `STRUO_BAZEL_PATH` to select a specific `bazel` executable when the
wrapper is active. The default resolution walks `PATH` and honors
[`bazelisk`](https://github.com/bazelbuild/bazelisk) shims.

See [`bazel_workspace`](/reference/prelude/bazel_workspace) and
[`bazel_target`](/reference/prelude/bazel_target) for their complete
contracts.

## Prerequisites

Install Bazel (or Bazelisk, which selects the version pinned by the
workspace) and verify it resolves in the shell that runs Struo:

```sh
bazel --version
```

The graph loader shells out to `bazel query` during resolution. A Bazel
workspace that cannot load through the raw `bazel` command cannot load
through Struo either. Diagnose loader errors with the same `bazel query`
invocation from the workspace root before revisiting Struo.
