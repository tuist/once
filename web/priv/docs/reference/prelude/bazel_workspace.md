# `bazel_workspace`

Native [Bazel](https://bazel.build) workspace seed.

## Description

`bazel_workspace` reads the Bazel graph through
[`bazel query`](https://bazel.build/query/language) and materializes every
rule the query returns as a `bazel_target` graph node. In this first
integration Bazel executes each rule so the workspace stays buildable from
day one; the direction of travel is to lower specific rule kinds into Once's
own target kinds (for example rules_rs `rust_binary` into Once's
[`rust_binary`](/reference/prelude/rust_binary)) so Once compiles them
directly, the way [`swift_package_workspace`](/reference/prelude/swift_package_workspace)
does for Apple targets. Rules that Once does not yet lower keep delegating
to Bazel.

Once discovers a Bazel workspace automatically from `MODULE.bazel`. A
repository without `once.toml` can therefore query and build its rules with
`once` as soon as the module is checked in. Discovery skips generated
directories such as `bazel-bin`, `bazel-out`, and `bazel-testlogs`.

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
The `once native show bazel` command prints the label-to-name mapping.

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

Discover and preview the workspace without writing a manifest:

```sh
once native list
once native show bazel
```

Store the generated seed only when it should be reviewed in `once.toml`:

```sh
once native init bazel
```

The imported seed is equivalent to:

```toml
[[target]]
name = "bazel"
kind = "bazel_workspace"
srcs = ["MODULE.bazel"]

[target.attrs]
resolver_inputs = [
  "MODULE.bazel",
  "WORKSPACE",
  "WORKSPACE.bazel",
  "MODULE.bazel.lock",
  "**/BUILD",
  "**/BUILD.bazel",
  "**/*.bzl",
]
```

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
