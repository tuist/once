# What is Once

Once is an execution layer for project scripts. It turns ordinary repository commands into content-addressed actions with explicit inputs, outputs, environment dependencies, working directories, and runtime metadata.

## Why this layer exists

Most repositories already have a long tail of scripts: test setup, asset generation, packaging, fixture updates, deployment hooks, local tools, and CI glue. They are important, but they usually sit outside any cacheable execution model. A developer or agent runs them again because there is no shared contract that says what changed.

Once gives those scripts that contract without asking teams to adopt a new build system.

::: tip Build systems can model this too
Build systems can model cacheable workflows with rich dependency graphs,
but moving existing repository automation into that shape can be a large
product and migration decision. Once is for the scripts you already have
and want to make cacheable without rewriting them as build rules.
:::

## How It Works

Start with a normal script. Keep the shebang, keep the implementation,
and add a few `# once` comments at the top:

```sh
#!/usr/bin/env bash
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

Then run the script through Once:

```sh
once exec -- bash scripts/build.sh
```

Once reads the script, hashes the declared contract, and either reuses a
previous result or runs the command and stores stdout, stderr, exit status,
and declared outputs. Workspace-level `once.toml` files are reserved for
shared configuration such as cache provider settings.

For longer-running workflows, Once exposes runtime sessions with structured
logs, events, and descriptors. That gives tools and agents a runtime API to
query instead of forcing them to scrape terminal output.

## What Once Is Not

Once is not trying to replace Buck, Bazel, Cargo, pnpm, Xcode, or language-specific build tools. Those systems should keep doing the work they are good at. Once focuses on the scripts and command workflows that teams already run around those tools, making them cacheable, inspectable, and portable.
