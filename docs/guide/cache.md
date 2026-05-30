# Cache

Fabrik stores blobs and action results in a local content-addressed
store. Every cacheable action uses that same store, whether it comes
from a build-system rule, a script rule, or `fabrik exec`.

When a remote cache provider is configured, the local CAS stays in
front. Local hits remain filesystem-fast, and misses can mirror their
blobs and action results to the remote provider so another machine can
restore them later.

## Local CAS Root

By default, Fabrik stores its local CAS under the user's XDG cache
directory:

```txt
<XDG_CACHE_HOME>/fabrik/cas
```

Set `FABRIK_CACHE_DIR` to choose the local CAS root explicitly:

```sh
FABRIK_CACHE_DIR=/cache/fabrik/cas fabrik build crates/app/app
```

The override applies to every cacheable action, including native build
rules and scripts.

## Cache Location In CI

On GitHub Actions, Fabrik detects known runner cache volumes when the
provider exposes a stable mounted filesystem. Namespace Cache Volumes
are detected automatically and Fabrik stores its CAS under
`/cache/fabrik/cas` when the `/cache` mount is present.

Some runner providers accelerate caching without exposing a mounted
filesystem to the job. Instead, they provide a faster backend for the
GitHub Actions cache protocol through the runner environment, a
compatible cache action, or object storage under the hood. Depot,
BuildJet, RunsOn, WarpBuild, and Blacksmith's default dependency cache
generally fall into that category. Those providers do not change
Fabrik's local CAS root automatically. Use `FABRIK_CACHE_DIR` when a
provider gives your job a mounted volume, for example a Blacksmith
Sticky Disk mounted at a path you chose.

Remote reuse works through Fabrik's cache provider, not through the CI
runner. A cache miss in CI can write to the fast local mounted volume
and mirror the same action result and blobs to remote storage. A local
developer running the same target can then fetch those blobs from the
remote provider and restore the declared outputs without running the
command again.

## GitHub Actions Cache Bridge

Fabrik can also expose a GitHub Actions cache-compatible endpoint to
tools it runs. Set `FABRIK_GITHUB_ACTIONS_CACHE_BRIDGE=1` before
invoking Fabrik:

```sh
FABRIK_GITHUB_ACTIONS_CACHE_BRIDGE=1 fabrik run images/build
```

For local cache misses, Fabrik starts an action-scoped cache service and
injects `ACTIONS_CACHE_URL` and `ACTIONS_RUNTIME_TOKEN` into the child
process environment. Tools inside that action that use the GitHub
Actions cache protocol can save and restore archives through Fabrik's
cache provider instead of talking to the CI runner cache directly.

The bridge currently targets the v1 GitHub Actions cache protocol used
by cache clients that honor `ACTIONS_CACHE_URL`. It is meant for nested
tool caches inside a Fabrik action. Workflow-level `uses: actions/cache`
steps still run outside Fabrik, so Fabrik cannot change their
environment unless the workflow explicitly routes them through Fabrik.
