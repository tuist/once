<p align="center">
  <img src="assets/header.png" alt="Once: Run once. Reuse everywhere." width="50%" />
</p>

<p align="center">
  <a href="https://github.com/tuist/once/actions/workflows/once.yml"><img src="https://github.com/tuist/once/actions/workflows/once.yml/badge.svg" alt="CI" /></a>
  <a href="https://github.com/tuist/once/releases/latest"><img src="https://img.shields.io/github/v/release/tuist/once?display_name=tag&sort=semver" alt="Latest release" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/tuist/once" alt="License" /></a>
</p>

# Once

Once makes project scripts cacheable, observable, and remotely executable. Declare the inputs, outputs, environment, and runtime contract once, then reuse the result locally, in CI, or on a compute provider.

## Quick Start

Declare a script in `once.toml`:

```toml
[[script]]
name = "build-assets"
argv = ["bash", "scripts/build-assets.sh"]
input = ["scripts/build-assets.sh", "assets/**/*"]
output = ["dist/"]
```

Run it through the cache:

```sh
once run build-assets
once run build-assets --remote --compute microsandbox
```

Scripts can also describe themselves with `ONCE` headers and run directly:

```sh
once exec --script bash scripts/build-assets.sh
```

## Documentation

Read the documentation at [once.tuist.dev](https://once.tuist.dev).

## License

[MIT](LICENSE).
