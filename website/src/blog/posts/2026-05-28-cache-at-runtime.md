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
fabrik cache hash <path> [<path> ...]     # digest a file, or combine many
fabrik cache hash --combine <d1> <d2> ... # combine already-computed digests
fabrik cache blob put <path>              # store bytes, print the digest
fabrik cache blob put --key <digest>      # store bytes under a chosen key
fabrik cache blob get <digest>            # fetch a content-addressed blob
fabrik cache blob get --key <digest>      # fetch a keyed blob
fabrik cache blob exists <digest>         # exit 0 on hit, 1 on miss
fabrik cache blob exists --key <digest>   # same, for the keyed namespace
fabrik cache action get <digest>          # look up an action result
fabrik cache action put <digest> ...      # record an action result
fabrik cache action forget <digest>       # drop an action result
fabrik cache stats                        # counts and size for each namespace
```

Everything is keyed by 256-bit [BLAKE3](https://github.com/BLAKE3-team/BLAKE3) digests. There are two blob namespaces. The default is content-addressed: `put` returns the hash of the bytes, and a lookup is guaranteed to return bytes that hash back to that digest. The second is keyed: `put --key` stores bytes under a digest you choose, and `get --key` / `exists --key` look them up there. The split is deliberate. Action results reference output blobs by their content hash, and we never want a keyed entry to be silently substituted for one of those; an attacker who could pre-stage a keyed blob would otherwise be able to poison cached outputs. Keeping the two namespaces explicit at the CLI makes that swap impossible.

Action results carry exit code, captured stdout and stderr digests, and any declared outputs by workspace path. The same surface the build graph uses for its own actions is now available to a shell script you wrote last Tuesday.

A nice consequence is that hashing and storing are separate. `cache hash` computes a digest without writing anything to the store. That matters when the input is a 2 GB tarball you only want to address, not persist. When you do want to persist, `cache blob put` returns the same digest and keeps the bytes.

## Why expose this at all

Build systems already do this internally. The reason we want it at the level of a script is that the boundary of "what the build system knows about" is artificial. A pre-commit hook, a test runner, a deploy script, a fixture generator: all of them are workflows that produce a result from a set of inputs. They deserve the same skip-if-unchanged behavior. Forcing teams to wrap every one of these as a custom rule or a plugin is a tax we do not think is necessary.

Once the cache is a primitive, scripts can do interesting things on their own. Control flow becomes a first-class user of the cache. If a previous run with the same inputs already succeeded, the script can decide to exit early. If a previous run produced an artifact, the script can restore it instead of regenerating it. The script stays a script, and the speedup comes for free from the same store that the build graph uses.

## Skipping a Vitest run when nothing changed

A test suite is the classic case. The result of running [Vitest](https://vitest.dev) over a clean tree depends on the source, the tests, the config, and the lockfile. If none of those changed since the last green run, there is no reason to run them again.

The action cache is the right fit here, because what we want to remember is whether a *run* succeeded, not an artifact. Here is the smallest version of that idea:

```sh
#!/usr/bin/env bash
set -euo pipefail

# Derive a single digest from every input that determines the outcome.
# `cache hash` operates on files, so we pipe a deterministic tar of the
# source directories through it and combine with the lockfile and config.
src_digest=$(tar --sort=name -cf - src test | fabrik cache hash)
config_digest=$(fabrik cache hash vitest.config.ts pnpm-lock.yaml)
action=$(fabrik cache hash --combine "$src_digest" "$config_digest")

# If the same inputs already produced a green run, skip.
if fabrik cache action get "$action" --format json | grep -q '"hit":true'; then
  echo "vitest: cached green run for these inputs, skipping."
  exit 0
fi

# Otherwise, run them.
pnpm vitest run

# Record success so the next invocation can short-circuit.
empty=$(printf '' | fabrik cache blob put)
fabrik cache action put "$action" \
  --exit-code 0 \
  --stdout "$empty" \
  --stderr "$empty"
```

`cache hash` with multiple paths hashes each file and combines the digests in order. For directories, pipe a sorted `tar` stream through `cache hash` so the digest is deterministic regardless of filesystem order. Run the script once and the tests execute. Run it a second time without touching the inputs and the script exits immediately. Change a single character in any file under `src`, `test`, `vitest.config.ts`, or `pnpm-lock.yaml`, and the digest changes, the cache misses, the tests run again.

You can extend the same shape to other test runners, linters, type checkers, anything whose result is a function of a set of files.

## Restoring node_modules without reinstalling

The other shape this enables is restoring an artifact instead of regenerating it. `npm install` is the canonical example. It is slow, the inputs are very stable, and the output is a folder you could have copied in a fraction of the time.

This is where `cache blob put --key`, `cache blob get --key`, and `cache blob exists --key` pay off. We compute a key from the inputs, ask whether a blob already lives under that key, pull it down if so, and otherwise install and store the new tarball back under the same key.

```sh
#!/usr/bin/env bash
set -euo pipefail

# The install is determined by these two files.
key=$(fabrik cache hash package.json package-lock.json)

# If we have a tarball for this combination, restore and exit.
if fabrik cache blob exists --key "$key"; then
  fabrik cache blob get --key "$key" | tar -xf -
  echo "node_modules: restored from cache."
  exit 0
fi

# Cache miss: install and store the tarball under the input-derived key.
npm install
tar -cf - node_modules | fabrik cache blob put --key "$key"
```

Three commands carry the whole flow: one to derive the key, one to probe, one to either pull or populate. No action wrapping, no stdout/stderr placeholders, no JSON to parse. The script reads top to bottom and means exactly what it says.

The same pattern works for `pip install`, `bundle install`, `cargo fetch`, any output a tool produces deterministically from a small set of input files. The script does the wrapping. The cache does the heavy lifting.

Keyed blobs are local-only in this first cut. The cross-machine version, where the first developer to install pays the cost and every teammate and CI runner after them restores the tarball through [Tuist](https://tuist.dev), needs the keyed namespace to travel through the remote tier the same way content-addressed blobs already do. That work is next, and we will write about it when it lands.

## A primitive, not a feature

The reason we like this shape is that we do not have to anticipate what people will use it for. A cache addressable from the shell is a primitive. The CI bootstrap script that does ten things in a row can wrap each of them. The fixture generator that takes a minute can become a one-liner the second time. The agent that runs the same workflow ten times across ten worktrees can stop redoing the same work in parallel.

If you build something on top of these commands, or you wish they did one more thing to make your workflow possible, [open an issue or a discussion in the repository](https://github.com/tuist/fabrik). We are collecting use cases as we go, and the next round of work on the cache surface will be informed by what people are actually trying to skip.
