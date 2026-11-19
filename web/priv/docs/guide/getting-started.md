# Getting Started

This guide installs Struo, runs one cacheable script, and shows how to choose
the next path for your repository.

## Installation

Install the current release with [mise](https://mise.jdx.dev/):

```sh
mise use -g "github:tuist/struo@$(mise latest github:tuist/struo)"
mise exec -- struo --version
```

Use `mise use github:tuist/struo@...` without `-g` when a repository should
pin its own Struo version in `mise.toml`.

The remaining examples assume that mise is active in your shell. If it is
not, prefix each `struo` command with `mise exec --`.

### Build From Source

Building the CLI only needs the pinned Rust toolchain, not every ecosystem tool
used by Struo's development and test suite. Clone Struo, then run from the parent
directory, outside the checkout so mise does not load its full tool list:

```sh
git clone https://github.com/tuist/struo.git
mise exec rust@1.99.0 -- cargo build --release -p struo-cli --manifest-path "$PWD/struo/Cargo.toml"
./struo/target/release/struo --version
```

On macOS, install Xcode's command-line tools first. On Linux, install the native
development libraries `libcap-ng-dev` and `libasound2-dev`; the resulting binary
also needs `libasound2` at runtime.

## Try an Existing Project

If you already have a working Cargo, Swift Package Manager, Xcode, or Bazel
project, try it before writing Struo configuration. Keep its existing toolchain
installed: Struo reuses the native project metadata, but does not replace the
compiler or SDK.

Start by discovering what Struo can build:

```sh
cd path/to/project
struo query targets
struo build
struo test
```

Run `struo lint` when the discovered graph includes a lint target. A project
without one reports that no lint targets are available, rather than silently
skipping analysis.

Struo recognizes the native workspace from its existing files. `struo build`,
`struo test`, and `struo lint` each default to the workspace's own targets: the
build fans out over every workspace-owned target that exposes `build`, the
test run selects first-party tests, and lint runs every workspace-owned
target that exposes `lint`. No `struo.toml` is created.

Pass `--all` to any of the three to include targets reached through resolved
dependencies; `struo lint --all` in particular will lint vendored third-party
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
mkdir struo-demo
cd struo-demo
mkdir -p scripts
```

Save the following as `scripts/greet.sh`:

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

Create `message.txt`, make the script executable, and run it:

```sh
printf 'hello from Struo\n' > message.txt
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

The second trailer contains `cache hit`. Struo reused the recorded result and
restored the declared output without running the script body again.

Prove that Struo can restore the output, not just remember that the command
succeeded:

```sh
rm build/greeting.txt
./scripts/greet.sh
cat build/greeting.txt
```

You should see another cache hit and `hello from Struo` in the restored file.
Now change the input:

```sh
printf 'hello again\n' > message.txt
./scripts/greet.sh
```

Struo reports a miss and writes `hello again`. You have now checked both sides
of the contract: unchanged inputs reuse the result, and changed inputs run the
work again.

## What Struo Learned

The three `# struo` lines form the action contract:

- `input` tells Struo which files affect the result.
- `output` tells Struo what to capture and restore.
- `cwd` chooses the working directory for the script.

The script itself is also part of the cache key. Changing either the script
or its declared input causes the work to run again. Paths in these headers are
relative to the script's directory, which is why `../message.txt` names the
file at the repository root.

Correct reuse depends on an honest contract. Declare every file and environment
variable that affects the result, and every output you need restored. Struo
cannot infer a dependency that the script reads but does not declare. Do not
cache commands whose purpose is an external side effect, such as publishing a
release or sending a notification.

## Choose Your Next Path

- Continue with [Scripted automation](/guide/scripted/) when you want to
  cache existing repository scripts with minimal changes.
- Continue with the [Graph guide](/guide/graph/) when you want typed targets,
  dependencies, and capabilities that Struo and coding agents can query. Its
  [Linting guide](/guide/graph/linting) turns analyzer reports into cacheable,
  normalized findings with an explicit failure policy.
- Continue with the [software development kit overview](/guide/sdk/) when an
  application needs direct access to Struo cache primitives.
- Read [Connect A Project](/guide/infrastructure/connect) when you are ready to
  create a remote project and share cache entries with your team, or
  [Infrastructure](/guide/infrastructure/) to run actions on another machine.
