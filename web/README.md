# Once Web

Phoenix application for the Once marketing site and the public scripts and cached commands registry.

## Running locally with Once

Bootstrap the repository's Once binary and materialize the dependency sources
pinned by `mix.lock`:

```sh
mise exec -- cargo build --locked --package once-cli
cd web
mise exec -- mix deps.get --check-locked
cd ..
```

Once resolves those sources into one cacheable target per locked package. Build
the development application from the repository root:

```sh
mise exec -- target/debug/once build web/application_dev
```

Start the Phoenix development server from the same first-class target:

```sh
mise exec -- target/debug/once run web/application_dev
```

Each worktree receives an isolated development port. Use the address printed
by Phoenix when the server starts. Set `PORT` when a specific port is needed.

## Checks

```sh
mise exec -- target/debug/once build web/application_test
mise exec -- target/debug/once run web/application_test
mise exec -- target/debug/once test web/tests
mise exec -- target/debug/once build web/application_prod
mise exec -- mix --cd web esbuild.install
mise exec -- target/debug/once build web/release
```

Compilation and locked package outputs use the configured Once cache. The test
target always runs its database setup and test suite, so a cached test result
cannot hide a database failure.

The continuous integration workflow builds the repository's Once command-line
interface from source before invoking these targets, so this integration does
not require a published Once release.
