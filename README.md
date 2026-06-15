<p align="center">
  <img src="assets/header.png" alt="Once: Run once. Reuse everywhere." width="100%" />
</p>

<p align="center">
  <a href="https://github.com/tuist/once/actions/workflows/once.yml"><img src="https://github.com/tuist/once/actions/workflows/once.yml/badge.svg" alt="CI" /></a>
  <a href="https://github.com/tuist/once/releases/latest"><img src="https://img.shields.io/github/v/release/tuist/once?display_name=tag&sort=semver" alt="Latest release" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/tuist/once" alt="License" /></a>
</p>

# Once

Once makes repository automation graph-aware, cacheable, observable, and remotely executable. Model work as targets with capabilities that lower into content-addressed actions. Existing scripts can join the same action model immediately through `once exec` or script targets, so teams can start without rewriting the automation they already have.

## Quick Start

Start by adapting an existing script. Add a small contract that names the files, outputs, environment, and working directory that shape the action:

```sh
#!/usr/bin/env bash
# once input "../assets/**/*"
# once output "../dist/"
# once cwd ".."

npm run build-assets
```

Run it as a cached action:

```sh
once exec -- bash scripts/build-assets.sh
once exec --remote --compute microsandbox -- bash scripts/build-assets.sh
```

Scripts can also run directly with a Once shebang:

```sh
#!/usr/bin/env -S once exec -- bash
```

When the workflow needs dependencies, multiple capabilities, typed validation, or agent-editable structure, move it into the build graph and let the rule lower the target into the same action substrate.

## Documentation

Read the documentation at [once.tuist.dev](https://once.tuist.dev).

## Integrations

Use the `once` crate when embedding Once in Rust applications:

```toml
[dependencies]
once = { git = "https://github.com/tuist/once" }
```

Release builds also publish `Once.xcframework.zip` for Apple platforms.
The framework exposes a small C ABI that Swift and Objective-C can call,
with JSON requests and responses for cache access.

## License

[MIT](LICENSE).
