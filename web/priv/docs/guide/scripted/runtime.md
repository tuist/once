---
next:
  text: Infrastructure
  link: /guide/infrastructure/
---

# Manual Cache Access

Annotated scripts are the usual way to cache automation because Once owns the
lookup, execution, and output restoration. Use `once cache` when an existing
tool must perform those steps itself or when you need to inspect the cache.

Once exposes two related stores:

- The blob cache stores bytes under their content digest.
- The action cache maps a digest of declared inputs to an exit code, captured
  streams, and optional output blobs.

An action result can therefore point to blobs without duplicating their bytes.

## Store And Retrieve A Blob

Put a file or standard input into the blob cache. The command prints its
[BLAKE3](https://www.blake3.io/) content digest:

```sh
digest=$(printf 'hello from Once\n' | once cache blob put)
printf '%s\n' "$digest"
```

Use that digest to check for the blob and retrieve it:

```sh
once cache blob exists "$digest"
once cache blob get "$digest"
```

`exists` exits with status zero when the blob is present and status one when it
is absent. In [JavaScript Object Notation
(JSON)](https://www.json.org/json-en.html) or Token-Oriented Object Notation
output, it always exits with status zero and reports whether the blob is
present.

## Record An Action Result

The action cache derives its key from ordered input declarations. This
[Bash](https://www.gnu.org/software/bash/) script runs tests with the
[npm package manager](https://www.npmjs.com/) and uses the same inputs for the
lookup and the write:

```sh
#!/usr/bin/env bash
set -eu

inputs=(
  --input src
  --input test
  --input package-lock.json
)

if once cache action get "${inputs[@]}" --if-success; then
  printf 'tests already passed for these inputs\n'
  exit 0
fi

npm test
once cache action put "${inputs[@]}"
```

`put` records exit code zero by default. The next lookup succeeds only when the
same ordered inputs produce a cache hit whose recorded exit code is zero.

Use an output mapping when the result includes an artifact:

```sh
artifact_inputs=(--input src --input package-lock.json)
artifact_digest=$(once cache blob put build/archive.tar)
once cache action put "${artifact_inputs[@]}" \
  --output build/archive.tar="$artifact_digest"
```

Read the action result with `--format json` to obtain the output digest, then
pass that digest to `once cache blob get` when restoring the artifact.

## Input Declarations

| Declaration | Meaning |
| --- | --- |
| `<path>` | A file or directory. Directory entries are sorted before hashing. |
| `path:<path>` | An explicit path form for names that contain `:`. |
| `value:<text>` | A literal value. |
| `env:<NAME>` | An environment variable name and value. An unset variable contributes an empty value. |
| `-` | Standard input. It may appear once across all inputs. |

Input order is significant. Declaring `a` followed by `b` produces a different
key from declaring `b` followed by `a`.

## Command Summary

| Command | Purpose |
| --- | --- |
| `once cache blob put [<path>]` | Stores bytes from a file or standard input and prints the digest. |
| `once cache blob get <digest>` | Retrieves bytes by digest. |
| `once cache blob exists <digest>` | Checks whether a digest is present. |
| `once cache action get --input <declaration> ...` | Looks up a result derived from ordered inputs. |
| `once cache action get ... --if-success` | Succeeds only for a hit whose recorded exit code is zero. |
| `once cache action put --input <declaration> ...` | Records an action result and optional stream or output digests. |
| `once cache action forget <digest>` | Removes an action result by its precomputed digest. |
| `once cache stats` | Reports entry counts and local disk usage. |

Blob reads and action lookups use the remote provider configured for the
workspace when the requested data is not available locally.

## Next

Continue to [Infrastructure](/guide/infrastructure/) to keep the local workflow
while sharing cache records with other machines. Use the
[cache command reference](/reference/cli/cache) for every flag and output
format.
