# Tuist

[Tuist](https://tuist.dev) lets Once share cache entries and execute commands
across machines. Start with the cache, verify that another checkout can reuse a
result, then add remote execution if the project needs it.

## Configure The Cache

Add a Tuist provider and cache binding to the repository root `once.toml`:

```toml
[infrastructures.tuist]
kind = "tuist"
account = "acme"
project = "app"

[infrastructure.cache]
provider = "tuist"
```

Replace `acme` and `app` with the Tuist account and project that should own the
shared entries.

## Authenticate A Developer Machine

Sign in once:

```sh
once auth login --provider tuist
```

Once reuses the stored session for cache reads and writes. Remove it when the
machine should no longer access the provider:

```sh
once auth logout --provider tuist
```

## Verify A Shared Result

Use two machines with the same `once.toml`, script, and input. On the first
machine, run the [example scripted workflow](/guide/scripted/):

```sh
./scripts/greet.sh
```

The first run reports a cache miss. On a second machine whose local cache has
not seen this result, create the same `message.txt` and run the script again:

```sh
printf 'hello from Once\n' > message.txt
./scripts/greet.sh
```

The second machine should report a cache hit and restore
`build/greeting.txt`. Because its local cache has no matching entry, the hit
confirms that Tuist supplied the recorded result.

## Authenticate Automation

Choose the authentication method supported by the environment.

### Account Token

Set `TUIST_TOKEN` to a Tuist account token with cache access when the runner can
store a long-lived secret:

```sh
TUIST_TOKEN=tuist_... ./scripts/greet.sh
```

### OpenID Connect

Use [OpenID Connect](https://openid.net/developers/how-connect-works/) on a
supported automation runner. Log in without opening a browser before running
cacheable commands:

```sh
once auth login --provider tuist --no-browser
./scripts/greet.sh
```

On GitHub Actions, grant identity-token permission before the login step:

```yaml
permissions:
  id-token: write
  contents: read

steps:
  - uses: actions/checkout@v6
  - run: once auth login --provider tuist --no-browser
  - run: ./scripts/greet.sh
```

The resulting session remains available for the rest of the job.

## Add Remote Execution

Bind execution to the same provider when commands should run through Tuist:

```toml
[infrastructure.execution]
provider = "tuist"
project = "preview-execution"
```

The `project` override is optional. It lets cache and execution use different
Tuist projects while sharing the provider's account and authentication.

Run a command remotely with the configured provider:

```sh
once exec --remote -- bash scripts/greet.sh
```

## Next

Read [Memory](/guide/memory/) to inspect the status, cache state, and action
identity that Once records for each run.
