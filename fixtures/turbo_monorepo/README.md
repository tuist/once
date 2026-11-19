# Turborepo acceptance fixture

An original, deliberately tiny pnpm workspace, not a copy of application code.
It retains task graph patterns observed in Turborepo's
[basic](https://github.com/vercel/turborepo/tree/main/examples/basic) and
[kitchen-sink](https://github.com/vercel/turborepo/tree/main/examples/kitchen-sink)
examples and in [create-t3-turbo](https://github.com/t3-oss/create-t3-turbo):

- `apps/*`, `packages/*`, and `tooling/*` workspace globs, with tools exposed to
  other packages through workspace `bin` links instead of registry packages.
- An internal package without a build script (`@acme/ui`) that consumers load
  from source, and a configuration package without any scripts.
- A package-level `turbo.json` that extends the root and replaces `outputs`
  with a negated cache directory, like Next.js applications' `.next/cache`.
- A `topo` transit task that only orders `lint` after upstream packages.
- A root `//#lint` task, `globalEnv`, per-task `env`, a `cache: false` deploy
  task, and a persistent `dev` task.
- `node --test` suites behind `test` tasks.

Only `turbo` comes from the registry, so `pnpm install` links the workspace
and fetches one package.
