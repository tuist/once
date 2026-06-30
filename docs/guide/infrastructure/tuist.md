# Tuist

[Tuist](https://tuist.dev) can provide Once remote cache and execution
configuration from the same named infrastructure provider. Once uses the Tuist
session to discover the closest cache endpoint and to talk to Tuist through the
[Bazel Remote Execution protocol](https://bazel.build/remote/remote-execution).

## Configure Tuist

Add a Tuist provider at the repository root:

```toml
[infrastructures.tuist]
kind = "tuist"
account = "acme"
project = "app"

[infrastructure.cache]
provider = "tuist"

[infrastructure.execution]
provider = "tuist"
project = "preview-execution"
```

The `account` identifies the Tuist account. The `project` identifies the Tuist
project used by default. Per-capability `project` fields override the provider
default, so cache and execution can share the same provider while using
different Tuist projects.

## Authentication

Tuist supports three authentication shapes.

### User Authentication

Use user authentication on developer machines. Sign in once before running
cacheable commands:

```sh
once auth login --provider tuist
```

Once stores the Tuist session and reuses it for cache reads, cache writes, and
endpoint discovery. To remove the stored session:

```sh
once auth logout --provider tuist
```

### Token

Set `TUIST_TOKEN` to a Tuist account token with cache access. Use this when the
environment can store a long-lived secret, for example a private Continuous
Integration ([CI](https://en.wikipedia.org/wiki/Continuous_integration)) runner
or a local automation machine.

```sh
TUIST_TOKEN=tuist_... once exec -- ./scripts/build.sh
```

### OpenID Connect

Use [OpenID Connect](https://openid.net/developers/how-connect-works/) on
supported Continuous Integration runners. Run
`once auth login --provider tuist --no-browser` before cacheable commands, and
Once exchanges the runner identity token with Tuist.

On GitHub Actions, grant identity-token permissions before the login step:

```yaml
permissions:
  id-token: write
  contents: read

steps:
  - uses: actions/checkout@v6
  - run: once auth login --provider tuist --no-browser
  - run: once exec -- ./scripts/build.sh
```

Once saves the resulting Tuist session for the rest of the job.
