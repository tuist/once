# Caching Scripted Workflows

The first scripted workflow declared an input, an output, and a working
directory. Add more headers only when another fact can change the result.

## Describe Every Relevant Input

Track files and directories with `input`. Track selected environment variables
with `env`, and use `fingerprint` for tool or host facts that are not files:

```sh
#!/usr/bin/env -S once exec -- bash
# once input "../src/**/*.ts"
# once input "../package-lock.json"
# once output "../dist/"
# once env "NODE_ENV"
# once fingerprint "node --version"
# once cwd ".."

npm run build
```

Here the source files, lockfile, `NODE_ENV` value, Node.js version, and script
file all contribute to the cache key. The script uses the
[npm package manager](https://www.npmjs.com/) to run the build. Once forwards
only declared environment variables. A fingerprint command runs before the
action; if it fails, Once stops before running the script.

::: tip Keep The Contract Precise
Declare facts that can change the result, not every file or host value the
script can see. A precise contract produces useful cache hits while still
invalidating the result when it should.
:::

## Connect Dependent Scripts

Use `needs` when one annotated script must run before another:

```sh
#!/usr/bin/env -S once exec -- bash
# once needs "./generate-assets.sh"
# once input "../src/**/*.ts"
# once output "../dist/"
# once cwd ".."

npm run build
```

Once runs independent dependencies concurrently. A dependency's declared
output fingerprints become part of the dependent script's cache key. If the
dependency runs again but produces the same outputs, the dependent script can
still reuse its previous result.

## Choose How Symbolic Links Are Stored

Directory outputs use `materialize-external` by default. When an output contains
a symbolic link whose target is outside that output directory, Once copies the
target into the cached result. This makes restored directories such as
`node_modules/` independent of a package manager's original storage location.

Use `output-symlinks` only when the script needs a different policy:

```sh
# once output "../dist/"
# once output-symlinks "preserve"
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

The script file itself is always part of the cache key, even when it has no
`input` headers.

## Use Another Script Runtime

The same headers work in languages that use line comments. Put the interpreter
after `once exec --` in the shebang:

::: code-group
```python [Python]
#!/usr/bin/env -S once exec -- python3
# once input "../src/**/*.py"
# once output "../dist/"

print("build")
```

```ruby [Ruby]
#!/usr/bin/env -S once exec -- ruby
# once input "../lib/**/*.rb"
# once output "../dist/"

puts "build"
```

```elixir [Elixir]
#!/usr/bin/env -S once exec -- elixir
# once input "../lib/**/*.ex"
# once output "../dist/"

IO.puts("build")
```
:::

Once also recognizes `//`, `;`, `--`, `%`, and `'` line-comment forms. To keep
an existing shebang, invoke the interpreter explicitly:

```sh
once exec -- python3 scripts/build.py
```

Once detects the headers and applies script-aware execution. Pass `--script`
when you need to force that mode.

## Use A Mise File Task

Once can wrap a [Mise file task](https://mise.jdx.dev/tasks/file-tasks.html)
without changing how callers invoke it:

```sh
#!/usr/bin/env -S once exec -- bash
#MISE description="Build assets"
# once input "../src/**/*.ts"
# once output "../dist/"

npm run build
```

After saving the task in a Mise task directory, run it normally:

```sh
mise run build
```

## Next

Configure [Infrastructure](/guide/infrastructure/) when these cacheable results
should be shared across machines. If an existing tool must probe or populate
the cache itself, continue to [Manual Cache Access](/guide/scripted/runtime).
