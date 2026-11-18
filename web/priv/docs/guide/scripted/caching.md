# Caching Scripted Workflows

The first scripted workflow declared an input, an output, and a working
directory. Add more headers only when another fact can change the result.

## Describe Every Relevant Input

Track files and directories with `input`. Track selected environment variables
with `env`, and use `fingerprint` for tool or host facts that are not files:

```sh
#!/usr/bin/env -S struo exec -- bash
# struo input "../src/**/*.ts"
# struo input "../package-lock.json"
# struo output "../dist/"
# struo env "NODE_ENV"
# struo fingerprint "node --version"
# struo cwd ".."

npm run build
```

Here the source files, lockfile, `NODE_ENV` value, Node.js version, and script
file all contribute to the cache key. The script uses the
[npm package manager](https://www.npmjs.com/) to run the build. Struo forwards
only declared environment variables. A fingerprint command runs before the
action; if it fails, Struo stops before running the script.

::: tip Keep The Contract Precise
Declare facts that can change the result, not every file or host value the
script can see. A precise contract produces useful cache hits while still
invalidating the result when it should.
:::

## Continue During a Cache Outage

If a remote cache remains unavailable after bounded retries, Struo executes the
script instead of reusing a cached result. Newly produced results remain in the
local cache, so later runs can reuse them even while the remote cache is down.
The outage is logged; it is not treated as proof that remote content is absent.

Builds can also reconstruct an unavailable deferred output when the current
build recorded its producer. Reconstruction must reproduce the exact expected
bytes and remains subject to resource admission and bounded repair limits.
An output without a recorded producer still fails if its bytes cannot be fetched.

Cache reuse and restoration still fail on rejected credentials, corrupt content,
malformed cache records, and local filesystem errors. Explicit cache reads, including SDK reads,
continue to report availability errors rather than silently return missing data.

## Connect Dependent Scripts

Use `needs` when one annotated script must run before another:

```sh
#!/usr/bin/env -S struo exec -- bash
# struo needs "./generate-assets.sh"
# struo input "../src/**/*.ts"
# struo output "../dist/"
# struo cwd ".."

npm run build
```

Struo runs independent dependencies concurrently. A dependency's declared
outputs become declared inputs of the dependent script, and their fingerprints
become part of its cache key. This lets a remote test consume an installation
script's `node_modules` output without exposing the rest of the host workspace.
If the dependency runs again but produces the same outputs, the dependent
script can still reuse its previous result.

## Check Repeated Execution

A second cache hit checks cache reuse, not whether executing the command again
produces the same result. To compare fresh executions of the greeting script
from [Scripted Workflows](/guide/scripted/), run:

```sh
struo exec --verify-reproducible --script -- bash scripts/greet.sh
```

Struo snapshots the whole workspace, then runs two trials from that same
snapshot at the same private workspace path. Each trial has its own initially
empty local Struo cache. Script dependencies declared with `needs` and commands
declared with `fingerprint` run inside each trial, rather than reusing work
performed before the check. Declared outputs stay in the private trials;
Struo does not replace outputs in your original workspace with trial artifacts.

The snapshot excludes `.struo` runtime state and known declared outputs of the
script and its dependencies. Outputs that overlap that script's declared inputs
stay in the snapshot, so in-place updates start from the same input in each trial.
Other pre-existing generated files remain part of
the starting snapshot. This is not a clean checkout or a way to discover every
undeclared input. Symlinks pointing outside the workspace and special files
prevent the snapshot from being made; materialize required files inside the
workspace or remove those entries before retrying. A script cannot declare the
whole workspace as an output for this check; declare its generated paths instead.

### Read The Result

Both trials must execute successfully. Two identical failures never pass.
Struo reports artifact equality separately from captured standard output and
standard error:

- The comparison covers the selected script's declared result, not artifacts
  and logs produced only by prerequisites. Prerequisites still run in each trial.
- When outputs are declared, artifact differences fail the check. Log-only
  differences are advisory and do not fail a matching-artifact check.
- Without declared artifacts, the check uses captured logs instead. Matching
  logs do not establish that undeclared files match.

The command exits successfully only when those conditions hold. A passing
check means **no artifact divergence observed in these two trials**, or matching
captured logs when no artifacts are declared. It is not proof of reproducibility.

### Inspect Retained Trials

The result prints a verification UUID and a command to inspect it again:

```sh
struo query verification <ID>
struo query verification <ID> --format json
```

These queries read the retained report without executing the script again.
The report is stored at `.struo/verifications/<ID>/report.json`. Each
`trial-1` and `trial-2` directory retains `result.json`, decoded declared
artifacts under `outputs/`, captured `stdout` and `stderr` when present, and
its local Struo cache. Failed trials also retain their workspace for debugging.
File-level artifact differences include bounded text excerpts and the first
differing byte offset when available. If the detail listing is truncated,
inspect the full retained artifacts instead. The
[verification report reference](/reference/mcp/#verification-reports) describes
the JSON fields used by command-line tools and coding harnesses.

Reports, artifacts, and logs are local runtime state, not automatically
published evidence. They can contain sensitive source or output; inspect them
before sharing and remove `.struo/verifications/<ID>/` when no longer needed.

### Know The Boundary

Only run a repeated-trial check when you authorize repeated execution of the
script and all its dependencies. Host toolchains, host tool caches, absolute
paths, network services, and external side effects are not isolated. Commands
using those resources can still change host or external state. Struo does not
perturb or control randomness, clocks, or thread scheduling, and both trials
use the same private path, so path-sensitive differences can remain hidden.
Script verification currently requires local execution, including every
`needs` dependency; scripts selecting remote execution are rejected.

To deny Internet sockets, including loopback, add `--network deny` to
`struo exec`. This policy also applies to `needs` dependencies and refuses
execution when enforcement is unavailable. Local Unix-domain communication
remains available. Fingerprint commands cannot currently enforce this policy,
so a script with a fingerprint fails closed under `--network deny`, rather
than executing the probe online. Replace the fingerprint with declared inputs
before running offline.

## Choose How Symbolic Links Are Stored

Directory outputs use `materialize-external` by default. When an output contains
a symbolic link whose target is outside that output directory, Struo copies the
target into the cached result. This makes restored directories such as
`node_modules/` independent of a package manager's original storage location.

Use `output-symlinks` only when the script needs a different policy:

```sh
# struo output "../dist/"
# struo output-symlinks "preserve"
```

`preserve` stores links as links. Use it only when the link targets will exist
at restore time.

## Supported Headers

| Header | Purpose |
| --- | --- |
| `input` | Tracks files, directories, or globs. |
| `needs` | Runs another annotated script first and includes its output fingerprints in this script's cache key. |
| `fingerprint` | Runs a read-only command and includes its text, exit status, standard output, and standard error in the cache key. |
| `output` | Declares files or directories to restore on cache hits. |
| `output-symlinks` | Uses `materialize-external` or `preserve` when capturing symbolic links in outputs. |
| `env` | Forwards a selected environment variable and includes its value in the cache key. |
| `cwd` | Chooses the working directory for the script body. |
| `remote` | Selects a named remote execution infrastructure provider. |

The script file itself is always part of the cache key, even when it has no
`input` headers.

## Use Another Script Runtime

The same headers work in languages that use line comments. Put the interpreter
after `struo exec --` in the shebang:

::: code-group
```python [Python]
#!/usr/bin/env -S struo exec -- python3
# struo input "../src/**/*.py"
# struo output "../dist/"

print("build")
```

```ruby [Ruby]
#!/usr/bin/env -S struo exec -- ruby
# struo input "../lib/**/*.rb"
# struo output "../dist/"

puts "build"
```

```elixir [Elixir]
#!/usr/bin/env -S struo exec -- elixir
# struo input "../lib/**/*.ex"
# struo output "../dist/"

IO.puts("build")
```
:::

Struo also recognizes `//`, `;`, `--`, `%`, and `'` line-comment forms. To keep
an existing shebang, invoke the interpreter explicitly:

```sh
struo exec -- python3 scripts/build.py
```

Struo detects the headers and applies script-aware execution. Pass `--script`
when you need to force that mode.

## Use A Mise File Task

Struo can wrap a [Mise file task](https://mise.jdx.dev/tasks/file-tasks.html)
without changing how callers invoke it:

```sh
#!/usr/bin/env -S struo exec -- bash
#MISE description="Build assets"
# struo input "../src/**/*.ts"
# struo output "../dist/"

npm run build
```

After saving the task in a Mise task directory, run it normally:

```sh
mise run build
```

## Next

Configure [Infrastructure](/guide/infrastructure/) when these cacheable results
should be shared across machines, or use
[Remote Execution](/guide/infrastructure/remote-execution) to run them in a
fresh sandbox. If an existing tool must probe or populate the cache itself,
continue to [Manual Cache Access](/guide/scripted/runtime).
