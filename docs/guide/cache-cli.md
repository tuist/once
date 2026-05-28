# Cache CLI

The `fabrik cache` CLI exposes Fabrik's content-addressed store
directly to shell scripts. Use it when you have a workflow whose
result is a function of a set of inputs, and you want to skip the
work, or restore an artifact, when those inputs have not changed.

This is the layer underneath [annotated script files](./cacheable-scripts.md).
Annotated scripts declare inputs and outputs once and let Fabrik handle
the cache around them. The Cache CLI is what you reach for when the
script needs to make the cache decisions itself, or when the work
being cached is not naturally an action.

## Commands

All commands live under `fabrik cache`.

| Command | Purpose |
| --- | --- |
| `fabrik cache hash <path> [<path> ...]` | Print the BLAKE3 digest of a file, or of several files combined in order. |
| `fabrik cache hash --combine <d1> <d2> ...` | Combine already-computed digests into one. |
| `fabrik cache blob put [<path>]` | Store bytes from a file (or stdin) and print their content digest. |
| `fabrik cache blob put --key <digest> [<path>]` | Store bytes under a caller-chosen digest in the keyed namespace. |
| `fabrik cache blob get <digest>` | Fetch bytes from the content-addressed namespace. |
| `fabrik cache blob get --key <digest>` | Fetch bytes from the keyed namespace. |
| `fabrik cache blob exists <digest>` | Exit 0 if the blob is present in the content-addressed namespace, 1 if not. |
| `fabrik cache blob exists --key <digest>` | Same, for the keyed namespace. |
| `fabrik cache action get <digest>` | Look up a cached action result. |
| `fabrik cache action put <digest> --exit-code <n> --stdout <d> --stderr <d> [--output path=digest ...]` | Record an action result. |
| `fabrik cache action forget <digest>` | Drop a cached action result. |
| `fabrik cache stats` | Print blob, keyed-blob, and action counts and on-disk size. |

All `--format json` and `--format toon` flags work as usual and return
machine-parseable output for scripts and agent consumers.

## Two blob namespaces

There are two blob namespaces, kept deliberately separate.

**Content-addressed blobs** (the default) are stored under the BLAKE3
hash of their bytes. A `get` always returns bytes that hash back to the
digest you asked for. The build graph and action results reference
output blobs by their content hash, and this invariant is what makes
caching safe.

**Keyed blobs** are stored under a digest you choose, typically derived
from a set of input files. The bytes are not required to hash to the
key. This is what lets a script memoize an artifact under a digest
derived from its inputs (for example, a `node_modules` tarball keyed by
the hash of `package.json` and the lockfile).

Keyed blobs are local-only today. Content-addressed blobs travel
through whatever remote infrastructure your `fabrik.toml` configures
(for example, [Tuist](https://tuist.dev)). Remote support for keyed
blobs is on the roadmap.

The two namespaces never overlap. `cache blob get <digest>` will never
return a keyed blob, and `cache blob get --key <digest>` will never
return a content-addressed blob. Passing `--key` is how you declare
which namespace you mean.

## Skip a test run when inputs have not changed

The action cache is the right primitive when what you want to remember
is whether a *run* succeeded.

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

## Restore an artifact instead of regenerating it

When what you want to remember is the *output* of a step (a tarball, a
generated folder, a built binary), the keyed-blob primitive is a
better fit. `npm install` is the canonical example.

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

## Exit codes and structured output

`cache blob exists` is designed to be used as a shell condition:

- In human mode, it exits `0` on hit and `1` on miss, with no stdout.
- In `--format json` or `--format toon`, it always exits `0` and emits
  `{ "digest": "...", "present": true|false }`.

`cache action get` always exits `0` whether the lookup hit or missed;
parse the `"hit"` field from `--format json` to branch.

`cache blob get` and `cache blob get --key` exit non-zero on miss with
an error on stderr.

## Hashing rules

- `cache hash <file>` prints the BLAKE3 of the file's bytes.
- `cache hash -` prints the BLAKE3 of stdin.
- `cache hash <file1> <file2> ...` hashes each file and combines the
  digests in order. This is shorthand for hashing each separately and
  passing them to `--combine`.
- `cache hash --combine <d1> <d2> ...` combines already-computed
  digests. The combination is order-sensitive: `combine(a, b) != combine(b, a)`.
- `-` may appear at most once. Passing `-` twice is rejected, since the
  second occurrence would read an already-drained stdin and silently
  hash to empty.

For directory inputs, pipe a deterministic `tar` stream through
`cache hash`:

```sh
tar --sort=name -cf - src test | fabrik cache hash
```
