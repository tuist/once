# What is Once

Once is an execution layer for project scripts. It turns ordinary repository commands into content-addressed actions with explicit inputs, outputs, environment dependencies, working directories, and runtime metadata.

::: warning Beta
Once is beta software. The CLI, local cache, remote execution path, and runtime API are usable, but the script contract may still change.
:::

## Why this layer exists

Most repositories already have a long tail of scripts: test setup, asset generation, packaging, fixture updates, deployment hooks, local tools, and CI glue. They are important, but they usually sit outside any cacheable execution model. A developer or agent runs them again because there is no shared contract that says what changed.

Once gives those scripts that contract without asking teams to adopt a new build system.

## What Once Standardizes

Once projects use `# Once` script headers to describe:

- command arguments
- input files and globs
- output paths
- tracked environment variables
- working directory
- timeout and resource hints
- optional remote compute provider
- runtime metadata for agent-facing sessions

That definition becomes the cache key and the remote execution request. If the inputs and execution contract have not changed, Once can reuse the previous result.

## Scripts Belong Here

The script stays in a checked-in file:

```sh
#!/usr/bin/env bash
# Once input "../assets/**/*"
# Once output "../dist/"
# Once cwd ".."

scripts/build-assets.sh
```

Once reads those headers when the script runs through `once exec`.
Workspace-level `once.toml` files are reserved for shared configuration
such as cache provider settings.

## Runtime API

Once also exposes runtime sessions for tools and agents that need structured access to command output and metadata. Instead of scraping a terminal log, an agent can query logs, events, and runtime descriptors over the local JSON-RPC control socket.

## What Once Is Not

Once is not trying to replace Buck, Bazel, Cargo, pnpm, Xcode, or language-specific build tools. Those systems should keep doing the work they are good at. Once focuses on the scripts and command workflows that teams already run around those tools, making them cacheable, inspectable, and portable.
