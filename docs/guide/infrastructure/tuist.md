# Tuist

[Tuist](https://tuist.dev) can provide Once cache storage and remote execution
configuration from the same named infrastructure provider. Once uses the Tuist
session to discover the closest cache endpoint and to talk to Tuist through the
[Bazel Remote Execution protocol](https://bazel.build/remote/remote-execution).

## Configure Tuist

Add a Tuist provider at the repository root:

```text
[infrastructure.cache]
provider = "tuist"

[infrastructure.execution]
provider = "tuist"
project = "preview-execution"

[infrastructures.tuist]
kind = "tuist"
account = "acme"
project = "app"
```

The `account` identifies the Tuist account. The `project` identifies the Tuist
project used by default. Per-capability `project` fields override the provider
default, so cache and execution can share the same provider while using
different Tuist projects.

## Authenticate Locally

Sign in once before running cacheable commands:

```sh
once auth login --provider tuist
```

Once stores the Tuist session and reuses it for cache reads, cache writes, and
endpoint discovery. To remove the stored session:

```sh
once auth logout --provider tuist
```

## Continuous Integration

For Continuous Integration
([CI](https://en.wikipedia.org/wiki/Continuous_integration)), Tuist accepts two
authentication shapes:

- Set `TUIST_TOKEN` to a Tuist account token with cache access.
- Run `once auth login --provider tuist --no-browser` on a runner that exposes
  an [OpenID Connect](https://openid.net/developers/how-connect-works/) identity
  token.

On GitHub Actions, grant identity-token permissions before the login step:

```text
permissions:
  id-token: write
  contents: read

steps:
  - uses: actions/checkout@v6
  - run: once auth login --provider tuist --no-browser
  - run: once exec -- ./scripts/build.sh
```

Once exchanges the runner identity token with Tuist and saves the resulting
Tuist session for the rest of the job.

Set `ONCE_CACHE_PROVIDER=local` on any step that should keep the repository
configuration but avoid remote cache traffic for that process:

```sh
ONCE_CACHE_PROVIDER=local once exec -- ./scripts/build.sh
```
