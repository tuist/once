# Connect A Project

`once connect` provisions a project with an infrastructure provider and records
the binding in your repository. It is the shortest path from a local project to
a shared remote project.

## One Step

With [Tuist](/guide/infrastructure/tuist) as the provider:

```sh
once connect --provider tuist --create
```

The command signs you in when needed, creates a project named after your
workspace directory under your personal account, and writes the provider
binding into the repository's root `once.toml`:

```toml
[infrastructure.cache]
provider = "tuist"

[infrastructures.tuist]
kind = "tuist"
url = "https://tuist.dev"
account = "acme"
project = "app"
```

That is the whole setup on the machine that ran the command. The manifest
itself holds no credentials: teammates and continuous integration still sign
in with `once auth login --provider tuist` (or a `TUIST_TOKEN`) before they can
read or write the shared project.

## Choose The Account And Name

Pass the account and project explicitly when the workspace name is not the
project handle you want, or when the project should live in an organization:

```sh
once connect --provider tuist --create --account acme --project app
```

When `--account` is omitted, the provider creates the project under the account
that owns your session, such as your personal account.

## Bind An Existing Project

Leave out `--create` to bind a project that already exists:

```sh
once connect --provider tuist --account acme --project app
```

With no account or project, Once lists the projects the connected session can
see. It binds automatically when there is exactly one and asks you to choose
when there are several.

## Preview And Verify

Print the binding without creating a project, signing in, or writing anything:

```sh
once connect --provider tuist --create --account acme --project app --dry-run
```

A dry run with `--create` needs an explicit `--account`, since the account is
only known once the project exists.

Pass `--format json` for a machine-readable result. Re-running the same command
is safe: Once resolves the binding already in the workspace before contacting
the provider, detects that it matches, and reports that the workspace is
connected without rewriting the file or creating a second project.

## Why It Is Provider Neutral

`once connect` does not name a provider. It resolves the provider the same way
`once auth login` does, asks that provider to create or list a project, and
writes the provider-neutral binding the rest of Once already reads. Tuist is the
first provider to implement project provisioning; a future provider plugs into
the same workflow without a new command.

## Next

Read [Tuist](/guide/infrastructure/tuist) to learn how shared cache entries are
verified across machines, then open [Infrastructure](/guide/infrastructure/) to
add an execution provider when commands should run away from your computer.
