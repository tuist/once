# What is Once

Once is an execution layer for project scripts. It turns ordinary repository commands into content-addressed actions with explicit inputs, outputs, environment dependencies, working directories, and runtime metadata.

## Why this layer exists

Most repositories already have a long tail of scripts: test setup, asset generation, packaging, fixture updates, deployment hooks, local tools, and CI glue. They are important, but they usually sit outside any cacheable execution model. A developer or agent runs them again because there is no shared contract that says what changed.

Once gives those scripts that contract without asking teams to adopt a new build system.

## What Once Standardizes

Once projects use `# once` script headers to describe:

- **Command arguments**: The runtime and script invocation that Once should execute.
- **Input files and globs**: The source files, directories, and patterns that participate in the cache key.
- **Output paths**: The files or directories Once should restore when a cache hit is available.
- **Tracked environment variables**: Host values that should be forwarded and included in the cache key.
- **Working directory**: The directory where the script should run.
- **Timeout and resource hints**: Execution limits that shape how Once schedules the action.
- **Remote compute provider**: The optional provider that can execute cache misses away from the local host.
- **Runtime metadata**: Agent-facing descriptors for sessions that expose logs, events, and controls.

That definition becomes the cache key and the remote execution request. If the inputs and execution contract have not changed, Once can reuse the previous result.

## Scripts Belong Here

The script stays in a checked-in file:

```sh
#!/usr/bin/env bash
# once input "../assets/**/*"
# once output "../dist/"
# once cwd ".."

scripts/build-assets.sh
```

Once reads those headers when the script runs through `once exec`.
Workspace-level `once.toml` files are reserved for shared configuration
such as cache provider settings.

## What Once Is Not

Once is not trying to replace Buck, Bazel, Cargo, pnpm, Xcode, or language-specific build tools. Those systems should keep doing the work they are good at. Once focuses on the scripts and command workflows that teams already run around those tools, making them cacheable, inspectable, and portable.
