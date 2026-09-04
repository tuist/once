# `bazel_workspace`

Native [Bazel](https://bazel.build) workspace seed.

## Description

`bazel_workspace` reads the Bazel graph through
[`bazel query`](https://bazel.build/query/language) and materializes every
rule the query returns as a `bazel_target` graph node. When a graph target
is built, Once reads Bazel's action graph for that target through
[`bazel aquery`](https://bazel.build/query/aquery) and runs every action
itself from a shadow execution root under
`<workspace>/.once/bazel-shadow/<target>/`. Bazel supplies the analysis
phase (target loading, toolchain resolution, external repository download
via `bazel fetch`, the action graph); Once supplies the execution phase.
`bazel build` is not invoked in the ownership path.

Some Bazel actions are implemented inside Bazel itself and have no argv in
`aquery` output (`Symlink`, `FileWrite`, `RunfilesTree`, `SymlinkTree`,
`RepoMappingManifest`). When a target's action graph contains any such
action, Once falls back to `bazel <capability>` for that target so it still
builds; the target's provider records which mnemonics forced the fallback
so users can see the gap. Each mnemonic Once learns to run natively shrinks
the fallback set until every Bazel rule executes through Once.

Once discovers a Bazel workspace automatically from `MODULE.bazel`,
`WORKSPACE.bazel`, or `WORKSPACE`. A repository without `once.toml` can
therefore query, build, and test its rules with `once` as soon as its native
workspace file is checked in. Discovery skips generated directories such as
`bazel-bin`, `bazel-out`, and `bazel-testlogs`.

The default query is `kind("rule", //...)`, which returns every buildable
rule the workspace exposes. The `query` attribute overrides that expression
when only a subset of the graph should reach Once. The `exclude_packages`
attribute drops named package prefixes from the default query without
rewriting it.

Each rule becomes one of three target kinds so the graph advertises only
the capabilities the underlying rule actually supports: `_test` rules become
[`bazel_test`](/reference/prelude/bazel_test) (build + test), `_binary`
rules become [`bazel_binary`](/reference/prelude/bazel_binary) (build + run),
and everything else becomes [`bazel_target`](/reference/prelude/bazel_target)
(build only). Every generated target records the original Bazel label on
`bazel_label` and its rule class on `bazel_rule_kind`. The graph node name
folds `/` and `:` into `_` and adds a `bz_` prefix so the label survives
Once's target-name grammar; two Bazel labels that would collide in that
mapping are reported as a resolver error instead of silently deduplicated.
`once query targets` prints the label-to-name mapping.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `bazel` | string | no | `bazel` | Bazel executable name or workspace-relative path. Resolves through [`bazelisk`](https://github.com/bazelbuild/bazelisk) when installed. |
| `query` | string | no | `kind("rule", //...)` | Bazel query expression used to enumerate rules. |
| `exclude_packages` | list&lt;string&gt; | no | `[]` | Package prefixes to strip from the default query. Each entry excludes both `//<prefix>/...` and `//<prefix>:*`. |
| `resolver_inputs` | list&lt;string&gt; | no | `srcs` | Package-relative source globs supplied during graph loading. |

## Providers and capabilities

The workspace target emits `bazel_workspace` and exposes the `build`
capability. The resolver emits one `bazel_target` per rule; those children
carry the actual build, test, and run capabilities.

## Direct use

Discover and use the workspace without writing a manifest:

```sh
once query targets
once build --ui
once test
```

Targetless build selects the single discovered workspace seed. Targetless
test selects the first-party Bazel test rules reported by its resolver.
Discovery does not create `once.toml`.

## Coexistence with Once outputs

Once writes its own cache and outputs under `.once/`. When Bazel walks the
workspace for a query, it also visits that directory unless the repository
lists it in a
[`.bazelignore`](https://bazel.build/reference/command-line-reference#flag--noenable_bzlmod)
file. Add a single line to `.bazelignore` alongside `MODULE.bazel`:

```
.once
```

Without that entry, a checked-in `BUILD.bazel` created by another native
integration can be picked up by `bazel query` and fail the resolver with a
missing rule loader.

## Sources

- [Bazel query language](https://bazel.build/query/language) describes the
  expressions used to enumerate rules during graph loading.
- [Bazel command-line reference](https://bazel.build/reference/command-line-reference)
  documents the `bazel build`, `bazel test`, and `bazel run` commands the
  target kind delegates to.
