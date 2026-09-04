---
prev: false
next: false
---

# Bazel

Once recognizes an existing Bazel workspace and can build or test it without
`once.toml`. Bazel remains responsible for evaluating its native graph,
downloading external repositories, and maintaining its own incremental cache.
Once places that work in the same scheduled and observable command surface as
the rest of a mixed workspace.

## Try an Existing Workspace

Install Bazel or Bazelisk, then verify that the repository's selected version
starts:

```sh
bazel --version
```

From a directory containing `MODULE.bazel` or `WORKSPACE`, run:

```sh
once query targets
once build --ui
once test
```

No initialization step or Once manifest is required. `once query targets`
shows the discovered `bazel_workspace` seed and its complete-workspace command
target. `once build --ui` runs `bazel build //...` and opens the Runs interface
for live progress. `once test` runs `bazel test //...` and records the Bazel log
as a normalized Once test result.

The first invocation can access the network when Bazel needs repositories or a
toolchain. Later invocations benefit from Bazel's own local cache. The Once
adapter is deliberately opaque: individual Bazel targets are not translated
into Once targets yet, so query and cache boundaries remain at the workspace
command level.

## Workspace Boundaries

A discovered Bazel root owns projects nested below it. This prevents Rust,
Swift, or other example projects stored inside a Bazel repository from becoming
competing default build roots. A repository containing several independent
Bazel roots still reports each root separately. Use the identifiers printed by
`once query targets` to select one when the targetless build is ambiguous.

## Choose an Explicit Target

The generated command target is named `bazel_all` inside its package. Build or
test it explicitly when a repository contains multiple native roots:

```sh
once build path/to/workspace/bazel_all
once test path/to/workspace/bazel_all
```

Use Bazel directly when you need a label narrower than `//...`, a configuration
flag, a query, or another Bazel subcommand. Once currently recognizes only the
complete default build and test forms.
