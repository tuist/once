# Scripts

Scripts are Once's adapter for existing executable automation. Keep the
file, keep the implementation, and add a small contract that tells Once
what the action reads, what it writes, and which host values affect the
result.

## Why Scripts

Most repositories already have a long tail of scripts: test setup, asset
generation, packaging, fixture updates, deployment hooks, local tools, and
CI glue. They are important, but they often sit outside any cacheable
execution model. A developer, CI job, or agent runs them again because
there is no shared contract that says what changed.

Once gives those scripts that contract without asking teams to move the
implementation into a new build-system shape. A script is the least typed
way to enter the same graph and action model that typed target kinds use.

::: tip Scripts are adapters
Use scripts when the workflow is still one opaque executable action. Move
the work into a typed graph target kind when it needs dependencies, multiple
capabilities, structured diagnostics, or agent-editable shape.
:::

## How It Works

Start with a normal script. Point the shebang at Once, keep the
implementation, and add a few `# once` comments at the top:

```sh
#!/usr/bin/env -S once exec -- bash
# once input "../src/**/*"
# once output "../dist/"
# once env "NODE_ENV"
# once cwd ".."

npm run build
```

Those comments are the script contract:

- **Inputs**: The files, directories, and globs that decide whether the work changed.
- **Outputs**: The files or directories Once should restore on a cache hit.
- **Environment variables**: Declared host values that are forwarded to the script and included in the cache key.
- **Working directory**: The directory where the script should run.

Then run the script directly:

```sh
./scripts/build.sh
```

Once reads the script, hashes the declared contract, and either reuses a
previous result or runs the command and stores stdout, stderr, exit status,
and declared outputs. Root `once.toml` files configure shared settings such as
cache providers, while package `once.toml` files may grow build graph
declarations as Once expands beyond scripts.

The root configuration can name a provider once and use it for more than one
capability. Per-capability fields override the provider defaults:

```text
[infrastructure.cache]
provider = "tuist"

[infrastructure.execution]
provider = "tuist"
project = "preview-execution"

[infrastructures.tuist]
kind = "tuist"
account = "acme"
project = "app"
```

In this example, cache and execution both use the `tuist` provider and the
same account. Cache uses the provider's default project, while execution sends
`preview-execution`. Pass `--compute <provider>` with `--remote` when a single
run should bypass the configured execution provider.

For Continuous Integration
([CI](https://en.wikipedia.org/wiki/Continuous_integration)), Tuist accepts two
authentication shapes. Set `TUIST_TOKEN` to an account token with cache scopes,
or run `once auth login --provider tuist` before cacheable commands on GitHub
Actions, CircleCI, or Bitrise. In those runners, Once requests the provider's
OpenID Connect identity token, exchanges it with Tuist, and saves the resulting
Tuist session for the rest of the job.

Set `ONCE_CACHE_PROVIDER=local` on a command when a runner should keep the
repository configuration but use only the local cache for that process.

```yaml
permissions:
  id-token: write
  contents: read

steps:
  - uses: actions/checkout@v5
  - run: once auth login --provider tuist
  - run: once exec -- ./scripts/build.sh
```

## Next

Read [Caching](/guide/scripts/caching) for annotation examples across
languages and [Runtime](/guide/scripts/runtime) for the `once cache` CLI
primitives that scripts can call directly.
