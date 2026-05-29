---
title: "The cache, at runtime"
description: "Fabrik now exposes its content-addressed cache as a small set of CLI commands so any script can check, store, and restore results without going through the build graph."
date: 2026-05-28
author: "The Fabrik team"
---

The cache inside a build system is usually only visible to the build system. The graph decides what counts as an action, the runner hashes its inputs, and the result lives in a store nobody else can reach. That works for the actions the build system already knows about. It does not help with everything else, which in most repositories is a lot.

A repository tends to grow a long tail of scripts that exist next to the build. Test runners. Codegen. Dependency installs. Environment bootstraps. They are the moments where a developer or an agent says "do this thing first, then move on". They are also, very often, exactly the moments where a cache would matter most, because the inputs barely changed and the work is about to run again.

## What we shipped

Fabrik already used a content-addressed cache for the actions it executes. With this work, that cache is reachable from any shell. A handful of commands, all under `fabrik cache`, do the small set of things a script needs to participate in caching on its own:

```sh
fabrik cache blob put <path>              # store bytes, print the content digest
fabrik cache blob get <digest>            # fetch a blob by content digest
fabrik cache blob exists <digest>         # exit 0 on hit, 1 on miss
fabrik cache action get --input <spec>... # look up a result by declared inputs
fabrik cache action put --input <spec>... # record a result
fabrik cache action forget <digest>       # drop a result
fabrik cache stats                        # counts and size for each cache
```

Two caches. The shape mirrors what Bazel and the [Remote Execution API](https://github.com/bazelbuild/remote-apis) settled on. The **blob cache** stores bytes addressed by their [BLAKE3](https://github.com/BLAKE3-team/BLAKE3) hash; a `get` always returns bytes that hash back to the digest you asked for. The **action cache** maps an action digest, which Fabrik derives from whatever inputs you declare, to an `ActionResult`: an exit code, optional stdout and stderr digests, and any declared outputs as `path -> blob digest`. Output digests point back into the blob cache by content, so an action result is a small proto and the bytes live in one place.

### Declaring inputs

Most commands accept inputs that describe what determines a result. The grammar is small and the same everywhere it appears:

| Spec | Meaning |
| --- | --- |
| `<path>` | A file or directory (directories walk sorted, content + relative path) |
| `path:<path>` | Explicit path form for names that contain `:` |
| `value:<str>` | A literal string |
| `env:<NAME>` | An environment variable, hashed as `<NAME>\0<value>` so two variables sharing a value do not collide |
| `-` | Standard input |

So when you write `--input src --input vitest.config.ts --input env:NODE_ENV`, Fabrik walks the `src/` tree sorted, hashes the config file's bytes, reads `NODE_ENV` from the environment, combines the three digests in order, and uses that as the action key. You never see a hash unless you ask for one.

## Why expose this at all

Build systems already do this internally. The reason we want it at the level of a script is that the boundary of "what the build system knows about" is artificial. A pre-commit hook, a test runner, a deploy script, a fixture generator: all of them are workflows that produce a result from a set of inputs. They deserve the same skip-if-unchanged behavior. Forcing teams to wrap every one of these as a custom rule or a plugin is a tax we do not think is necessary.

Once the cache is a primitive, scripts can do interesting things on their own. Control flow becomes a first-class user of the cache. If a previous run with the same inputs already succeeded, the script can decide to exit early. If a previous run produced an artifact, the script can restore it instead of regenerating it. The script stays a script, and the speedup comes for free from the same store that the build graph uses.

## Skipping a Vitest run when nothing changed

A test suite is the classic case. The result of running [Vitest](https://vitest.dev) over a clean tree depends on the source, the tests, the config, and the lockfile. If none of those changed since the last green run, there is no reason to run them again.

What we want to remember is whether a *run* succeeded, not an artifact, so the action cache is the right fit:

```sh
#!/usr/bin/env bash
set -euo pipefail

inputs=(
  --input src
  --input test
  --input vitest.config.ts
  --input pnpm-lock.yaml
)

# `--if-success` exits 0 only when the cache has a record AND the
# recorded exit code is 0. A cached failure or a miss exits non-zero,
# and the tests run.
if fabrik cache action get "${inputs[@]}" --if-success; then
  echo "vitest: cached green run for these inputs, skipping."
  exit 0
fi

pnpm vitest run

# Record success. `put` defaults --exit-code to 0; `set -e` would have
# exited above on failure, so the only path that reaches this line is
# a green run.
fabrik cache action put "${inputs[@]}"
```

Run the script once and the tests execute. Run it a second time without touching the inputs and the script exits immediately. Change a single character in any file under `src`, `test`, `vitest.config.ts`, or `pnpm-lock.yaml`, and the input digest changes, the cache misses, the tests run again.

You can extend the same shape to other test runners, linters, type checkers, anything whose result is a function of a set of files.

## Restoring node_modules without reinstalling

The other shape this enables is restoring an artifact instead of regenerating it. `npm install` is the canonical example. It is slow, the inputs are very stable, and the output is a folder you could have copied in a fraction of the time.

The recipe is the same primitive at work, with one extra step: the artifact you want to remember lives in the blob cache, and the action result you record under the input digest points at it.

```sh
#!/usr/bin/env bash
set -euo pipefail

inputs=(--input package.json --input package-lock.json)

# If we recorded a result for these inputs, restore the tarball.
result=$(fabrik cache action get "${inputs[@]}" --format json)
if echo "$result" | grep -q '"hit":true'; then
  digest=$(echo "$result" | jq -r '.result.outputs["node_modules.tar"]')
  fabrik cache blob get "$digest" | tar -xf -
  echo "node_modules: restored from cache."
  exit 0
fi

# Cache miss: install, store the tarball in the blob cache, and
# record an action result pointing at it.
npm install
nm_digest=$(tar -cf - node_modules | fabrik cache blob put)
fabrik cache action put "${inputs[@]}" \
  --output node_modules.tar="$nm_digest"
```

Three operations carry the whole flow: probe the action cache, put the tarball in the blob cache, record the result. Because the tarball is content-addressed, two teammates whose installs produce byte-identical bytes end up sharing one entry; the second teammate's `cache blob put` recognises the digest and the bytes are not duplicated.

The same pattern works for `pip install`, `bundle install`, `cargo fetch`, any output a tool produces deterministically from a small set of input files. The script does the wrapping. The cache does the heavy lifting.

The cross-machine version, where the first developer to install pays the cost and every teammate and CI runner after them restores the tarball through [Tuist](https://tuist.dev), already works for the blob cache; the action-cache side of the same shape lands as that integration matures.

## A mise toolchain that follows you between branches

[mise](https://mise.jdx.dev) is a popular runtime version manager. Switching branches whose `mise.toml` declares different tool versions kicks off a fresh round of downloads even when you have already paid for them on another branch or another machine. The [mise-action](https://github.com/jdx/mise-action) for GitHub Actions exists for exactly this reason: it caches `~/.local/share/mise/` (the binary plus every installed tool) under a key derived from the platform, the mise version, and the mise config and lockfile, and restores it on the next run.

The shape isn't GitHub-specific. The same script gives a single dev machine, an agent's worktree, and a CI runner the same speedup — and on a Tuist-backed blob cache, the toolchain follows you between machines too.

```sh
#!/usr/bin/env bash
set -euo pipefail

# Platform discriminator first: a mise tree built on macOS arm64 cannot
# run on Linux x86_64. `value:` carries a cache-format version you bump
# when the layout itself changes.
inputs=(
  --input value:mise-v1
  --input env:OSTYPE
  --input env:HOSTTYPE
  --input mise.toml
  --input mise.lock
)

mise_dir="${MISE_DATA_DIR:-$HOME/.local/share/mise}"

result=$(fabrik cache action get "${inputs[@]}" --format json)
if echo "$result" | grep -q '"hit":true'; then
  digest=$(echo "$result" | jq -r '.result.outputs["mise.tar"]')
  mkdir -p "$mise_dir"
  fabrik cache blob get "$digest" | tar -xzf - -C "$(dirname "$mise_dir")"
  echo "mise: restored toolchain from cache."
else
  mise install --locked
  tools_digest=$(
    tar --sort=name -czf - -C "$(dirname "$mise_dir")" "$(basename "$mise_dir")" \
      | fabrik cache blob put
  )
  fabrik cache action put "${inputs[@]}" --output mise.tar="$tools_digest"
fi
```

The pattern generalises: anything that mutates a known directory deterministically from a small set of inputs (a virtualenv built from `requirements.txt`, a Bundler gem set, a sandboxed npm workspace, a `cargo build` target directory) fits the same three commands.

## A primitive, not a feature

The reason we like this shape is that we do not have to anticipate what people will use it for. A cache addressable from the shell is a primitive. The CI bootstrap script that does ten things in a row can wrap each of them. The fixture generator that takes a minute can become a one-liner the second time. The agent that runs the same workflow ten times across ten worktrees can stop redoing the same work in parallel.

If you build something on top of these commands, or you wish they did one more thing to make your workflow possible, [open an issue or a discussion in the repository](https://github.com/tuist/fabrik). We are collecting use cases as we go, and the next round of work on the cache surface will be informed by what people are actually trying to skip.
