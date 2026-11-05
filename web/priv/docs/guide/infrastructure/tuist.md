# Tuist

[Tuist](https://tuist.dev) provides shared caching, live run reporting and
artifact publication for Once, with application previews for supported native
interfaces. Configure the capabilities your project needs
independently.

::: info Trigger orchestration is coming
Tuist support for [Once triggers](/guide/infrastructure/triggers) is coming.
This page covers caching, run reporting and application previews, not CI
orchestration.
:::

## Create And Connect A Project

Provision a Tuist project and bind this repository to it in one command:

```sh
once connect --provider tuist --create
```

Once signs you in when needed, creates the project under your personal account,
and writes the provider binding into the repository root `once.toml`. Read
[Connect A Project](/guide/infrastructure/connect) for account and project
selection. The result is the same configuration described below.

## Configure The Cache

Add a Tuist provider and cache binding to the repository root `once.toml`:

```toml
[infrastructures.tuist]
kind = "tuist"
account = "acme"
project = "app"

[infrastructure.cache]
provider = "tuist"
```

Replace `acme` and `app` with the Tuist account and project that should own the
shared entries.

## Authenticate A Developer Machine

Sign in once:

```sh
once auth login --provider tuist
```

Once reuses the stored session for cache reads, cache writes and artifact
publication. Remove it when the machine should no longer access the provider:

```sh
once auth logout --provider tuist
```

## Verify A Shared Result

Use two machines with the same `once.toml`, script, and input. On the first
machine, run the [example scripted workflow](/guide/scripted/):

```sh
./scripts/greet.sh
```

The first run reports a cache miss. On a second machine whose local cache has
not seen this result, create the same `message.txt` and run the script again:

```sh
printf 'hello from Once\n' > message.txt
./scripts/greet.sh
```

The second machine should report a cache hit and restore
`build/greeting.txt`. Because its local cache has no matching entry, the hit
confirms that Tuist supplied the recorded result.

## Publish Application Previews

Configure Tuist as the artifact publication provider in the repository root
`once.toml`. If the project already defines this provider for caching, reuse
that definition and add only the artifact binding:

```toml
[infrastructures.tuist]
kind = "tuist"
account = "acme"
project = "application"

[infrastructure.artifacts]
name = "tuist"
```

```sh
once auth login --provider tuist
once artifact publish MyApplication --track review
```

Tuist receives the full immutable artifact closure through its authenticated
project artifact service, not a selected preview payload. The adapter uses
existing Tuist authentication and streams content without inspecting native
executables. A matching remote receipt confirms publication and returns the
project's previews URL. Preview projection happens asynchronously, so this
receipt does not mean an application is already available to install.

To publish after successful builds, add an explicit policy and authorize this
machine and workspace:

```toml
[infrastructure]
publish_on_build = ["MyApplication"]
```

```sh
once artifact publish MyApplication --allow-on-build
once build MyApplication
```

All requested builds must succeed, and the producer must approve the entire
closure for automatic publication. No artifact-publication request occurs when
the policy or local authorization is absent. Authorization binds the exact
endpoint and target to Tuist's stable project destination, `project:<project_id>`.
Changing the endpoint, workspace or target requires new authorization; a changed
stable destination is rejected before upload. See
[Artifacts](/guide/infrastructure/artifacts#publish-after-successful-builds)
for the provider-neutral policy and replay behavior.

### Supported Formats And Limits

The receiver retains arbitrary artifact payloads and unknown interfaces without
executing them. Metadata documents are limited to one mebibyte and the full
closure to 20 GiB. Known native interfaces additionally create previews using
the project's default preview visibility and existing Tuist distribution surfaces.
Generic artifact contents remain private even in public projects:

- `apple_application` exports `dev.once.apple-install.v2` on macOS, alongside
  its v1 installation description. The v2 interface points to the bundle ZIP
  and `spec.metadata_payload` with `once.apple_application.v1` metadata.
  Ad-hoc signed macOS and simulator applications are eligible for automatic
  publication and preview projection. Physical-device applications are not.
- `android_binary` exports `dev.once.android-install.v1`, pointing to the APK
  and `spec.metadata_payload` with `once.android_application.v1` metadata.
  Debug-signed, non-instrumentation APKs approve automatic publication.
  Unsigned APKs do not produce installable previews.

Native producers validate signatures and collect metadata; the generic adapter
does not. Unknown interfaces remain available through authenticated artifact
downloads even when no preview is projected.

The same intent is looked up remotely before transfer. A verified retained
receipt avoids another upload; an unconfirmed completion can be recovered by
replaying the same intent. Publications expire after 30 days on hosted Tuist,
or according to the self-hosted app-preview retention setting. Expired records
remain as tombstones and cannot be reactivated by replaying the same intent.
The returned URL is not a permanent-byte availability guarantee.

## Follow Runs Live

Once streams build and test runs to Tuist when the provider is configured and
the machine is authenticated. Each run publishes ordered event batches to the
project's account and project, and the service returns a dashboard link that
Once prints as the run starts. Open it to follow the graph, cache decisions,
and captured output while the run progresses.

Live reporting is best-effort: a missing endpoint or an expired session is
logged and never fails the run. Re-authenticate with `once auth login
--provider tuist` when the session expires.

## Authenticate Automation

Choose the authentication method supported by the environment.

### Account Token

Set `TUIST_TOKEN` to a Tuist account token authorized for the project's operations
when the runner can store a long-lived secret:

```sh
TUIST_TOKEN=tuist_... ./scripts/greet.sh
```

### OpenID Connect

Use [OpenID Connect](https://openid.net/developers/how-connect-works/) on a
supported automation runner. Log in without opening a browser before running
cacheable commands:

```sh
once auth login --provider tuist --no-browser
./scripts/greet.sh
```

On GitHub Actions, grant identity-token permission before the login step:

```yaml
permissions:
  id-token: write
  contents: read

steps:
  - uses: actions/checkout@v6
  - run: once auth login --provider tuist --no-browser
  - run: ./scripts/greet.sh
```

The resulting session remains available for the rest of the job.

## Connection recovery

Once retries transient cache failures during blob existence checks, uploads, and
action-result publication with at most three attempts per RPC. This includes
HTTP/2 connection retirement, where the cache asks the client to use a new
connection. Interrupted uploads restart from the beginning with a fresh upload
resource and a newly opened file or stream.

Authentication failures, permission refusals, invalid requests, quota refusals,
requests that exceed the request timeout, and local file errors are not
retried, so a cache that stops responding fails after one timeout rather than
three. Retries do not provide resumable uploads.

## Run Actions On Tuist Workers

When remote execution is enabled for the account, Tuist runs graph command
actions on single-use Linux workers through the Bazel Remote Execution API.
Once uploads only the input files the project cache does not already hold,
asks the service to run the action, and reads outputs back from the same cache.
Dependencies still decide when each action starts, and results stay in the
project cache for later builds and other machines.

Pin the worker image the project's execution service runs and select the same
Tuist infrastructure for caching and execution:

```toml
[infrastructure.cache]
provider = "tuist"

[infrastructure.execution]
provider = "tuist"

[infrastructures.tuist]
kind = "tuist"
account = "acme"
project = "app"
execution_image = "sha-REPLACE_WITH_THE_IMAGE_TAG"
```

```sh
once build --remote your_target
```

The credential needs read and write access to the project. Once fails before
uploading anything when the account has no execution capacity, when the cache
and execution name different projects, or when `execution_image` differs from
the image the service advertises. Changing the image partitions build
configuration and cached results.

The Tuist server must advertise the worker image, the cache endpoint, the
account's execution concurrency, and the absolute directory where workers run
actions. Once fails before uploading anything against a server that omits the
execution directory, because it cannot place outputs and working directories
safely without it.

Once keeps at most as many actions in flight as the account's advertised
concurrency. When the service answers that its queue is full, Once waits with
backoff and submits again instead of failing. If the service rejects the
credential during a long build, Once refreshes it once and retries.

Workers run `linux/amd64`. Commands must resolve their tools inside the worker
image, not on the computer that started the build, and declared outputs must
sit beneath the command's working directory. Workers look up a bare program
name such as `sh` only through the action's `PATH`. When an action declares no
`PATH`, Once sends `/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin`,
the default of most Linux images. An action that times out on a worker fails as
a timeout and its logs are still shown. The service may run an action
more than once, for example after it loses an operation during a restart, so
place only actions that are safe to repeat on it. Unsupported requirements,
such as network denial or a timeout longer than ten minutes, fail instead of
running locally.

Inputs go to the cache endpoint the execution service advertises. When
`TUIST_CACHE_ENDPOINT` is set, Once uses it instead and logs a warning if it
differs, so set it only to an endpoint the workers read from.

## Next

Add an [execution provider](/guide/infrastructure/remote-execution) when the
command should also run away from the current computer. Read
[Memory](/guide/memory/) to inspect the status, cache state, and action identity
that Once records for each run.
