# Scripted Workflows

Scripted workflows bring existing automation into Once without rewriting it.
The script remains the source of truth. A small header tells Once which inputs
affect the result, which outputs to preserve, and which host values belong in
the cache key.

This is the shortest path from an ordinary script to a cacheable Once action.

## Create A Cacheable Script

Suppose a repository turns `message.txt` into `build/greeting.txt`. Save this
script as `scripts/greet.sh`:

```sh
#!/usr/bin/env -S once exec -- bash
# once input "../message.txt"
# once output "../build/greeting.txt"
# once cwd ".."

set -eu
mkdir -p build
cp message.txt build/greeting.txt
cat build/greeting.txt
```

The `once` headers form the cache contract:

- `input` tracks a file, directory, or glob that can change the result.
- `output` identifies a file or directory that Once should restore on a cache
  hit.
- `cwd` chooses the working directory for the script body.

Paths in the headers are relative to the script file, which keeps the contract
stable when the repository moves.

Make the script executable, create its input, and run it:

```sh
chmod +x scripts/greet.sh
printf 'hello from Once\n' > message.txt
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
