# Scripted

Scripted is Once's adapter for existing executable automation. Keep the
file, keep the implementation, and add a small contract that tells Once
what the action reads, what it writes, and which host values affect the
result.

## Why Scripted

Most repositories already have a long tail of scripts: test setup, asset
generation, packaging, fixture updates, deployment hooks, local tools, and
CI glue. They are important, but they often sit outside any cacheable
execution model. A developer, CI job, or agent runs them again because
there is no shared contract that says what changed.

Once gives those scripts that contract without asking teams to move the
implementation into a new build-system shape. A script is the least typed
way to enter the same graph and action model that typed target kinds use.

::: tip Scripted workflows are adapters
Use scripts when the workflow is still one opaque executable action. Move
the work into a typed graph target kind when it needs multiple capabilities,
structured diagnostics, or an agent-editable shape.
:::

## How It Works

Start with a normal script. Point the shebang at Once, keep the
implementation, and add a few `# once` comments at the top:

```sh
#!/usr/bin/env -S once exec -- bash
# once input "../src/**/*"
# once output "../dist/"
# once env "NODE_ENV"
# once fingerprint "node --version"
# once cwd ".."

npm run build
```

Those comments are the script contract:

- **Inputs**: The files, directories, and globs that decide whether the work changed.
- **Outputs**: The files or directories Once should restore on a cache hit.
- **Environment variables**: Declared host values that are forwarded to the script and included in the cache key.
- **Fingerprints**: Read-only commands whose output should affect the cache key.
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

See [Infrastructure](/guide/infrastructure/) for remote cache and execution
provider configuration.

## Next

Read [Caching](/guide/scripted/caching) for annotation examples across
languages and [Runtime contract](/guide/scripted/runtime) for the
`once cache` primitives that scripts can call directly.
