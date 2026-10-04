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

## Try an Existing Project

If you already have a working Cargo, Swift Package Manager, Xcode, or Bazel
project, try it before writing Once configuration. Keep its existing toolchain
installed: Once reuses the native project metadata, but does not replace the
compiler or SDK.

Start by discovering what Once can build:

```sh
cd path/to/project
once query targets
once build
once test
```

Run `once lint` when the discovered graph includes a lint target. A project
without one reports that no lint targets are available, rather than silently
skipping analysis.

Once recognizes the native workspace from its existing files. `once build`,
`once test`, and `once lint` each default to the workspace's own targets: the
build fans out over every workspace-owned target that exposes `build`, the
test run selects first-party tests, and lint runs every workspace-owned
target that exposes `lint`. No `once.toml` is created.

Pass `--all` to any of the three to include targets reached through resolved
dependencies; `once lint --all` in particular will lint vendored third-party
code, so pair it with `--fail-on error` when you want to keep the noise
manageable. See [Graph guide](/guide/graph/) for the full workspace-owned
rule (and its current heuristic limitations) and the per-ecosystem behavior.

Continue with the matching guide for [Rust](/guide/graph/rust), [Swift
Packages](/guide/graph/swift-packages), [Xcode
Projects](/guide/graph/apple/xcode), or [Bazel](/guide/graph/bazel) for
prerequisites, native dependency behavior, and current boundaries.

## Run a Cacheable Script

For a self-contained example, create an empty directory and a place for the
script. You can also follow these steps in an existing repository:

```sh
mkdir once-demo
cd once-demo
mkdir -p scripts
```

Save the following as `scripts/greet.sh`:

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

The first invocation ends with a trailer containing `cache miss`. The local
cache works without an account or a remote service. Run the
same command again:

```sh
./scripts/greet.sh
```

The second trailer contains `cache hit`. Once reused the recorded result and
restored the declared output without running the script body again.

Prove that Once can restore the output, not just remember that the command
succeeded:

```sh
rm build/greeting.txt
./scripts/greet.sh
cat build/greeting.txt
```

You should see another cache hit and `hello from Once` in the restored file.
Now change the input:

```sh
printf 'hello again\n' > message.txt
./scripts/greet.sh
```

Once reports a miss and writes `hello again`. You have now checked both sides
of the contract: unchanged inputs reuse the result, and changed inputs run the
work again.

## What Once Learned

The three `# once` lines form the action contract:

- `input` tells Once which files affect the result.
- `output` tells Once what to capture and restore.
- `cwd` chooses the working directory for the script.

The script itself is also part of the cache key. Changing either the script
or its declared input causes the work to run again. Paths in these headers are
relative to the script's directory, which is why `../message.txt` names the
file at the repository root.

Correct reuse depends on an honest contract. Declare every file and environment
variable that affects the result, and every output you need restored. Once
cannot infer a dependency that the script reads but does not declare. Do not
cache commands whose purpose is an external side effect, such as publishing a
release or sending a notification.

## Choose Your Next Path

- Continue with [Scripted automation](/guide/scripted/) when you want to
  cache existing repository scripts with minimal changes.
- Continue with the [Graph guide](/guide/graph/) when you want typed targets,
  dependencies, and capabilities that Once and coding agents can query. Its
  [Linting guide](/guide/graph/linting) turns analyzer reports into cacheable,
  normalized findings with an explicit failure policy.
- Continue with the [software development kit overview](/guide/sdk/) when an
  application needs direct access to Once cache primitives.
- Read [Connect A Project](/guide/infrastructure/connect) when you are ready to
  create a remote project and share cache entries with your team, or
  [Infrastructure](/guide/infrastructure/) to run actions on another machine.
