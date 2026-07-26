# Getting Started

This guide installs Once, runs one cacheable script, and shows how to choose
the next path for your repository.

## Installation

Install the current release with [mise](https://mise.jdx.dev/):

```sh
mise use -g "github:tuist/once@$(mise latest github:tuist/once)"
mise exec -- once --version
```

Use `mise use github:tuist/once@...` without `-g` when a repository should
pin its own Once version in `mise.toml`.

The remaining examples assume that mise is active in your shell. If it is
not, prefix each `once` command with `mise exec --`.

## Run A Cacheable Script

Create `scripts/greet.sh` in a repository:

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

Create `message.txt`, make the script executable, and run it:

```sh
printf 'hello from Once\n' > message.txt
chmod +x scripts/greet.sh
./scripts/greet.sh
cat build/greeting.txt
```

The first invocation ends with a trailer containing `cache miss`. Run the
same command again:

```sh
./scripts/greet.sh
```

The second trailer contains `cache hit`. Once reused the recorded result and
restored the declared output without running the script body again.

Change `message.txt` and run the command once more. The input changed, so
Once reports another miss and records the new output.

## What Once Learned

The three `# once` lines form the action contract:

- `input` tells Once which files affect the result.
- `output` tells Once what to capture and restore.
- `cwd` chooses the working directory for the script.

The script itself is also part of the cache key. Changing either the script
or its declared input causes the work to run again.

## Choose Your Next Path

- Continue with [Scripted automation](/guide/scripted/) when you want to
  cache existing repository scripts with minimal changes.
- Continue with the [Graph guide](/guide/graph/) when you want typed targets,
  dependencies, and capabilities that Once and coding agents can query.
- Continue with the [software development kit overview](/guide/sdk/) when an
  application needs direct access to Once cache primitives.
- Read [Infrastructure](/guide/infrastructure/) after the local flow works
  and you are ready to share cache entries or run actions remotely.
