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

Script files can also opt into remote execution:

```sh
#!/usr/bin/env bash
# FABRIK input "../src/**/*.ts"
# FABRIK output "../dist/"
# FABRIK remote "microsandbox"

npm run build
```

## Inline Script Configuration

Use `argv` when the action is easier to keep in `fabrik.toml`:

```toml
[[target]]
name = "lint"
rule = "script"

[target.script]
argv = ["pnpm", "eslint", "src/"]
input = ["src/**/*.ts"]
remote = "microsandbox"
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
| `remote` | Runs the script on a compute provider instead of the local host. |

## Remotely Executable Scripts

A remotely executable script is a script whose execution contract is
local and whose compute placement is configurable. The script still
declares the same `input`, `output`, `env`, and `cwd` surfaces. Fabrik
uses those declarations to compute the action key and restore declared
outputs from cache. The difference is that a cache miss can run on a
compute provider.

Today Fabrik recognizes `microsandbox` as the embedded development
provider and `daytona` as a remote workspace provider:

```toml
[[target]]
name = "bundle"
rule = "script"

[target.script]
argv = ["pnpm", "bundle"]
input = ["src/**/*.ts"]
output = ["dist/"]
remote = "microsandbox"
```

You can also choose remote execution from the CLI without changing the
manifest:

```sh
fabrik run --remote --compute microsandbox scripts/bundle
fabrik exec --remote --compute microsandbox -- bash scripts/bundle.sh
fabrik run --remote --compute daytona scripts/bundle
```

The Microsandbox adapter is linked into the Fabrik binary. It creates a
fresh local microVM, bind-mounts the workspace at `/workspace`, runs the
command there, then stops and removes the sandbox state before returning.
Set `FABRIK_MICROSANDBOX_IMAGE` to choose a different image, or
`FABRIK_MICROSANDBOX_WORKDIR` to mount the repository somewhere other
than `/workspace`.

The Daytona adapter talks to the Daytona API directly. Set
`FABRIK_DAYTONA_SANDBOX` to the sandbox id or name, and set
`FABRIK_DAYTONA_API_KEY` or `DAYTONA_API_KEY` before running. Set
`FABRIK_DAYTONA_WORKDIR` when the repository lives somewhere other than
`/workspace` inside the sandbox. Self-hosted or proxied deployments can
override the API endpoint with `FABRIK_DAYTONA_API_URL`:

```sh
export FABRIK_DAYTONA_SANDBOX=my-sandbox
export FABRIK_DAYTONA_API_KEY=...
export FABRIK_DAYTONA_WORKDIR=/workspace/fabrik
fabrik run --remote --compute daytona scripts/bundle
```

When the action is a cache miss, stdout and stderr stream through
Fabrik as the provider produces them, so the command behaves like a
local run from the caller's point of view. Cache hits replay the cached
stdout, stderr, exit code, and declared outputs.

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
