# Cache CLI

Most repositories have a long tail of scripts that exist next to the
build: test runners, codegen, dependency installs, environment
bootstraps. They are usually invisible to caching because they live
outside the formal graph, and so they run from scratch every time, on
every machine, even when their inputs have not changed.

The cost of that adds up quickly. A test suite that takes ninety
seconds runs ten times a day during a refactor. `npm install` re-runs
on every CI job, even when the lockfile is identical. An agent
operating across ten parallel worktrees redoes the same codegen step
ten times because none of the workers know the others already paid
for it.

The `fabrik cache` CLI exposes Fabrik's content-addressed store
directly to those scripts so they can stop. Hash the inputs that
determine a result, ask the cache whether you have already produced
that result, and either skip the work or restore the artifact. The
script stays a script. The speedup comes for free from the same store
that the build graph uses.

The surface is small. It has three caches and a hashing helper.

## Hashing

`cache hash` computes BLAKE3 digests without storing anything. Use it
to derive the keys you will then pass to the cache commands below.

| Command | Purpose |
| --- | --- |
| `fabrik cache hash <path>` | Print the BLAKE3 of a file's bytes. |
| `fabrik cache hash -` | Print the BLAKE3 of stdin. |
| `fabrik cache hash <path1> <path2> ...` | Hash each file and combine the digests in order. |
| `fabrik cache hash --combine <d1> <d2> ...` | Combine already-computed digests in order. |

A few rules worth knowing:

- Combination is order-sensitive: `combine(a, b) != combine(b, a)`.
- `-` may appear at most once. A second `-` would read an
  already-drained stdin and silently hash to empty, so it is rejected.
- For directory inputs, pipe a deterministic `tar` stream:
  `tar --sort=name -cf - src test | fabrik cache hash`.

## Content-addressed cache

The default blob namespace. Blobs are stored under the BLAKE3 hash of
their bytes. A `get` always returns bytes that hash back to the digest
you asked for. The build graph and action results reference output
blobs by their content hash, and this invariant is what makes caching
safe.

| Command | Purpose |
| --- | --- |
| `fabrik cache blob put [<path>]` | Store bytes from a file (or stdin) and print their content digest. |
| `fabrik cache blob get <digest>` | Fetch bytes by digest. |
| `fabrik cache blob exists <digest>` | Exit 0 on hit, 1 on miss. With `--format json`/`toon`, always exit 0 and emit `{"digest":"...","present":true|false}`. |

This namespace travels through whatever remote infrastructure your
`fabrik.toml` configures (for example, [Tuist](https://tuist.dev)).
`get` falls back to the remote on local miss; `exists` consults the
remote too, so the two are symmetric.

## Keyed cache

Blobs stored under a digest you choose, typically derived from a set
of input files. The bytes are not required to hash to the key. This
is what lets a script memoize an artifact under a digest derived from
its inputs (for example, a `node_modules` tarball keyed by the hash
of `package.json` and the lockfile).

| Command | Purpose |
| --- | --- |
| `fabrik cache blob put --key <digest> [<path>]` | Store bytes under the caller-chosen `<digest>`. |
| `fabrik cache blob get --key <digest>` | Fetch bytes by key. |
| `fabrik cache blob exists --key <digest>` | Exit 0 on hit, 1 on miss. |

Keyed blobs are kept in a separate namespace from content-addressed
blobs. `cache blob get <digest>` will never return a keyed blob, and
`cache blob get --key <digest>` will never return a content-addressed
blob. Passing `--key` is how you declare which namespace you mean. The
split is deliberate: action results reference output blobs by their
content hash, and we never want a keyed entry to be silently
substituted for one of those.

Keyed blobs are local-only today. Remote support is on the roadmap.

## Action cache

Maps an action digest to an `ActionResult`: the captured exit code,
stdout and stderr digests, and any declared outputs (workspace path →
blob digest). This is the right primitive when what you want to
remember is whether a *run* succeeded, not an artifact you produced.
The terminology mirrors the Bazel/REAPI action cache.

| Command | Purpose |
| --- | --- |
| `fabrik cache action get <digest>` | Look up a cached action result. Always exits 0; parse `--format json` for `"hit": true\|false`. |
| `fabrik cache action put <digest> --exit-code <n> --stdout <d> --stderr <d> [--output path=digest ...]` | Record an action result. |
| `fabrik cache action forget <digest>` | Drop a cached action result. |

## Inspecting

| Command | Purpose |
| --- | --- |
| `fabrik cache stats` | Print counts and on-disk size for the content-addressed, keyed, and action caches. |

## Examples

### Skip a test run when inputs have not changed

```sh
#!/usr/bin/env bash
set -euo pipefail

# Derive a single digest from every input that determines the outcome.
# `cache hash` operates on files; pipe a deterministic tar of any source
# directories through it so directory iteration order does not leak in.
src_digest=$(tar --sort=name -cf - src test | fabrik cache hash)
config_digest=$(fabrik cache hash vitest.config.ts pnpm-lock.yaml)
action=$(fabrik cache hash --combine "$src_digest" "$config_digest")

# If the same inputs already produced a green run, skip.
if fabrik cache action get "$action" --format json | grep -q '"hit":true'; then
  echo "vitest: cached green run for these inputs, skipping."
  exit 0
fi

pnpm vitest run

# Record success so the next invocation can short-circuit.
empty=$(printf '' | fabrik cache blob put)
fabrik cache action put "$action" \
  --exit-code 0 \
  --stdout "$empty" \
  --stderr "$empty"
```

The same shape works for any test runner, linter, type checker, or
codegen step whose result is a deterministic function of a set of
files.

### Restore an artifact instead of regenerating it

When what you want to remember is the *output* of a step (a tarball,
a generated folder, a built binary), the keyed cache is a better fit.
`npm install` is the canonical example.

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

Three commands carry the whole flow: one to derive the key, one to
probe, one to either pull or populate.

The same pattern works for `pip install`, `bundle install`,
`cargo fetch`, or any output a tool produces deterministically from a
small set of input files.
