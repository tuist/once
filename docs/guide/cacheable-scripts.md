# Script Files

Script files are the easiest way to plug existing repository automation
into Fabrik's execution model without rewriting the implementation.

Most repositories already have a layer of shell, Node, Python, Ruby, or
Elixir automation that does real work but lives outside the native build
graph. Fabrik lets those scripts stay where they are. The script file
remains the source of truth, and Fabrik learns just enough about it to
cache it, inspect it, and schedule it safely.

## Annotated Script Files

Use a script file when the implementation belongs in a real script.
The annotation model works across common scripting languages. For
example:

::: code-group
```sh [Bash]
#!/usr/bin/env bash
# FABRIK input "../src/**/*.ts"
# FABRIK output "../dist/"
# FABRIK env "NODE_ENV"

npm run build
```

```python [Python]
#!/usr/bin/env python3
# FABRIK input "../src/**/*.py"
# FABRIK output "../dist/"
# FABRIK env "PYTHONPATH"

print("build")
```

```ruby [Ruby]
#!/usr/bin/env ruby
# FABRIK input "../lib/**/*.rb"
# FABRIK output "../dist/"
# FABRIK env "RUBYLIB"

puts "build"
```

```elixir [Elixir]
#!/usr/bin/env elixir
# FABRIK input "../lib/**/*.ex"
# FABRIK output "../dist/"
# FABRIK env "MIX_ENV"

IO.puts("build")
```
:::

Ruby, Python, and Elixir all use `#` comments, so the annotations read
naturally in those files. Fabrik also accepts other line-comment forms
such as `//`, `;`, `--`, `%`, and `'` for languages that use them.

## Running Through Fabrik

To run an annotated script file through Fabrik, use the runtime you
would normally use locally and put it after `fabrik exec --`:

```sh
fabrik exec -- bash scripts/build.sh
```

If the file carries `FABRIK` headers, Fabrik automatically switches to
script-aware execution. The explicit `--script` form still works when
you want to force that mode.

## Executable Fabrik Shebangs

If you want the file itself to execute through Fabrik, use a Fabrik
shebang and name the runtime there:

::: code-group
```sh [Bash]
#!/usr/bin/env -S fabrik exec -- bash
# FABRIK input "../src/**/*.ts"
# FABRIK output "../dist/"
# FABRIK env "NODE_ENV"

npm run build
```

```python [Python]
#!/usr/bin/env -S fabrik exec -- python3
# FABRIK input "../src/**/*.py"
# FABRIK output "../dist/"
# FABRIK env "PYTHONPATH"

print("build")
```

```ruby [Ruby]
#!/usr/bin/env -S fabrik exec -- ruby
# FABRIK input "../lib/**/*.rb"
# FABRIK output "../dist/"
# FABRIK env "RUBYLIB"

puts "build"
```

```elixir [Elixir]
#!/usr/bin/env -S fabrik exec -- elixir
# FABRIK input "../lib/**/*.ex"
# FABRIK output "../dist/"
# FABRIK env "MIX_ENV"

IO.puts("build")
```
:::

This keeps the script directly executable while still letting Fabrik
apply the annotated cache contract.

The `FABRIK` directives at the top of the file describe the parts
Fabrik must reason about: tracked inputs, declared outputs, forwarded
environment variables, and a working directory. That keeps the
implementation and the cache contract in one place.

## Supported Annotations

| Annotation | Purpose |
| --- | --- |
| `input` | Declares tracked files, directories, or globs. |
| `output` | Declares output files or directories that Fabrik should restore on cache hits. |
| `env` | Forwards selected environment variables from the host and includes them in the cache key. |
| `cwd` | Chooses the working directory for the script. |

The script file itself is always part of the cache key, even when no
`input` directives are present.

::: tip Path Resolution
`FABRIK` paths are resolved relative to the script file's directory,
not relative to `fabrik.toml`. That keeps the script portable when it
lives in `scripts/`, `tools/`, or any other subdirectory next to the
code it automates.
:::
