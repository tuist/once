# Script Rules

Fabrik supports script actions through the built-in `script` rule. In
`fabrik.toml`, that rule can either point at a checked-in script file
or declare the action inline.

Run either form with `fabrik run`. This page focuses on the rule
surface in `fabrik.toml`. If you want the deeper walkthrough for
checked-in script files and `FABRIK` headers, start with
[Script Files](/guide/cacheable-scripts).

## Referencing A Script File

Use `path` when the implementation lives in a real script file:

```toml
[[target]]
name = "bundle"
rule = "script"

[target.script]
path = "scripts/bundle.sh"
```

In this form, the rule just points at the script file. The execution
contract lives in the file itself through `FABRIK` headers. That means
the rule-side surface is intentionally small: `path` tells Fabrik which
file to run, and the script file carries the inputs, outputs,
environment dependencies, and working directory.

## Inline Script Configuration

Use `argv` when the action is easier to keep in `fabrik.toml`:

```toml
[[target]]
name = "lint"
rule = "script"

[target.script]
argv = ["pnpm", "eslint", "src/"]
input = ["src/**/*.ts"]
```

This form is best when the action is compact and easier to review as
manifest data than as a separate script file. The body stays inline, but
the execution model is the same one Fabrik uses for file-backed scripts.
The rule fields use the same contract names you write in `FABRIK`
headers: tracked `input`, declared `output`, forwarded `env`, and `cwd`.

## Rule Fields

| Field | Purpose |
| --- | --- |
| `path` | Selects a checked-in script file. |
| `argv` | Declares an inline script action directly in `fabrik.toml`. |
| `input` | Declares tracked files, directories, or globs for the script. |
| `output` | Declares output files or directories that Fabrik should restore on cache hits. |
| `env` | Forwards selected environment variables from the host and includes them in the cache key. |
| `cwd` | Chooses the working directory for the script. |

## Migrating From `[[task]]`

Older manifests used `[[task]]` for the same kind of operational work.
Rewrite those entries as `[[target]]` with `rule = "script"` and
`[target.script]`.

```toml
[[task]]
name = "lint"
argv = ["pnpm", "eslint", "src/"]
src_globs = ["src/**/*.ts"]
outputs = [".fabrik/out/eslint.json"]
```

becomes:

```toml
[[target]]
name = "lint"
rule = "script"

[target.script]
argv = ["pnpm", "eslint", "src/"]
input = ["src/**/*.ts"]
output = [".fabrik/out/eslint.json"]
```
