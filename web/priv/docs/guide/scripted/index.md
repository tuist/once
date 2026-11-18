# Scripted Workflows

Scripted workflows bring existing automation into Struo without rewriting it.
The script remains the source of truth. A small header tells Struo which inputs
affect the result, which outputs to preserve, and which host values belong in
the cache key.

This is the shortest path from an ordinary script to a cacheable Struo action.

## Create A Cacheable Script

Suppose a repository turns `message.txt` into `build/greeting.txt`. Save this
script as `scripts/greet.sh`:

```sh
#!/usr/bin/env -S struo exec -- bash
# struo input "../message.txt"
# struo output "../build/greeting.txt"
# struo cwd ".."

set -eu
mkdir -p build
cp message.txt build/greeting.txt
cat build/greeting.txt
```

The `struo` headers form the cache contract:

- `input` tracks a file, directory, or glob that can change the result.
- `output` identifies a file or directory that Struo should restore on a cache
  hit.
- `cwd` chooses the working directory for the script body.

Paths in the headers are relative to the script file, which keeps the contract
stable when the repository moves.

Make the script executable, create its input, and run it:

```sh
chmod +x scripts/greet.sh
printf 'hello from Struo\n' > message.txt
./scripts/greet.sh
```

The first run executes the script and reports a cache miss. Run the same command
again:

```sh
./scripts/greet.sh
```

The second run reports a cache hit and restores `build/greeting.txt` from the
recorded result. If `message.txt` changes, the input digest changes and the
script runs again.

## Inspect and Run Through a Coding Harness

Inspect the parsed contract before execution:

```sh
struo query script scripts/greet.sh
```

Coding agents can use the same workflow without reading this guide. The
[Model Context Protocol](https://modelcontextprotocol.io/) server exposes
`struo_validate_script` for contract inspection and, when started with
`--allow-run`, `struo_exec_script` for execution. The execution result
includes the action digest, cache state, captured streams, and matching
evidence. Calling it twice with unchanged inputs verifies cache reuse.

## When To Use A Script

Use a scripted workflow when one existing executable already owns the work,
such as asset generation, test setup, packaging, or fixture updates. Move to a
[typed graph target](/guide/graph/) when the work needs multiple capabilities,
structured validation, or a shape that tools can inspect and edit.

## Next

Continue to [Caching](/guide/scripted/caching) to add environment variables,
tool fingerprints, and dependencies to the script contract. Read
[Manual Cache Access](/guide/scripted/runtime) only when a script needs to
manage cache records itself.
