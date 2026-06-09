# Once Web

Phoenix application for the Once marketing site and the public rules and scripts registry.

## Running Locally

Use the repository toolchain through mise:

```sh
mise exec -- mix setup
mise exec -- mix phx.server
```

The app is served at [localhost:4000](http://localhost:4000).

## Checks

```sh
mise exec -- mix precommit
```
