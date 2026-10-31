# Docker Execution

Docker provides a local way to exercise the same action boundary used by hosted
execution providers. With `once build --remote`, Once schedules ready compiler
actions independently, transfers their declared inputs, and collects their
declared outputs. Dependencies still determine when each action can start.

## Prepare An Immutable Environment

Install Docker and prepare an image containing the action's tools, `sleep`,
`mkdir`, `rm`, and `tar` with `--hard-dereference` support, such as GNU tar.
Pull or build the image yourself, then obtain its local immutable image ID:

```sh
docker image inspect --format '{{.Id}}' your-build-image
```

Configure that exact `sha256:` ID in the root `once.toml`:

```toml
[infrastructures.workers]
kind = "docker"
image = "sha256:REPLACE_WITH_THE_64_CHARACTER_IMAGE_ID"

[infrastructure.execution]
provider = "workers"
```

Tags and shortened IDs are rejected. Once does not pull images implicitly.
Changing the image changes the execution binding and partitions cached results.
The example value above is a placeholder, not a runnable image ID.

## Build Through The Provider

```sh
once build --remote --compute workers your_target
```

Omit `--compute workers` when the default execution provider is configured.
Omit `--remote` for the existing local build path. `--remote` cannot be combined
with reproducibility trials.

The build header names the selected provider. Add `-v` to see each completed
action's name, target, execution or cache status, and duration.

Graph analysis still runs on the coordinator. Generic toolchains need matching
coordinator and worker tool paths. For Apple targets, an image-bound toolchain
description supports Xcode resolution on macOS with compilation on Linux, as
described below. Compilation target and worker operating system are separate:
cross-compilation requires an appropriate toolchain and SDK in the image.

The coordinator needs access to Docker. Action containers receive neither a
host workspace mount nor the Docker socket. Do not give untrusted code control
of the coordinator or its Docker access. Docker is a development execution
backend, not a hostile multi-tenant isolation guarantee.

## Resource And Failure Behavior

Docker workers share Once's host admission budget with local actions and also
reserve capacity against the Docker daemon's reported CPUs and two thirds of
its memory. This accounts for Docker VM capacity when it is smaller than the
coordinator's memory. Provider aliases using the same daemon share this worker
budget within the process. Containers receive the admitted worker CPU and
memory limits, including a matching memory-plus-swap limit, and reservations
remain held through deletion. Target kinds can declare command resources with
[`run_action`](/reference/modules/); undeclared requests use Once's defaults.
If deletion fails, host CPU, memory, exclusive slots, and daemon reservations
remain held while background reconciliation retries cleanup. Reconciliation
does not need to acquire the capacity held by the orphaned container.

Each action snapshots its Docker endpoint and TLS configuration. Changing the
current Docker context does not redirect that action or its cleanup. Ownership
is tied to the daemon identity; an empty listing from another daemon cannot
release a live worker's reservation.

Once creates a fresh writable container for each uncached command, validates
returned outputs before installing them, and awaits container cleanup. Failed
or cancelled operations retain ownership records when cleanup is uncertain.
Declared `clean_paths` and `create_dirs` apply inside the worker after input
staging; private coordinator scratch contents are not copied as implicit inputs.
Response files are restored after cleanup. Ordered actions receive prior
same-target outputs unless they explicitly opt into independent input staging.
Unsupported action policies fail rather than silently running locally or
weakening their requirements. Cache hits can restore outputs without starting
a container.

Archive transfers enforce both progress-stall limits and a separate deadline
for the worker's acknowledgement after upload completes. Diagnostic output
does not count as artifact progress. The absolute artifact-transfer deadline
still bounds transfers that continue making progress.

## Resolve Xcode On Mac And Compile On Linux

Keep the original Xcode project and its `xcode_workspace` target on the Mac.
Once resolves native targets and source-package dependencies there, then sends
declared sources, headers, generated files, and dependency outputs to workers.
There is no graph export step or manual source checkout inside each worker.
Only required declared inputs are transferred, not the entire working tree.

The immutable Linux image must already contain Linux-compatible Swift, Clang,
Darwin SDK tools and resources, and `rcodesign`. Copying Xcode's macOS compiler
executables into Linux is not sufficient. Prepare the SDK according to its
licensing terms; Once does not redistribute Xcode or upload the SDK per action.

Describe paths **inside that exact image**, not paths on the Mac:

```toml
[infrastructure.execution]
provider = "workers"

[infrastructure.execution.properties.apple]
schema_version = 1
environment = "sha256:REPLACE_WITH_THE_64_CHARACTER_IMAGE_ID"
os = "linux"
arch = "arm64"
bin = "/usr/bin"
developer_dir = "/opt/darwin/Developer"
tools = "/opt/darwin/toolset/bin"

[infrastructure.execution.properties.apple.programs]
rcodesign = "/usr/local/bin/rcodesign"

[infrastructure.execution.properties.apple.sdks.iphonesimulator]
sdk_path = "/opt/darwin/Developer/Platforms/iPhoneSimulator.platform/Developer/SDKs/iPhoneSimulator.sdk"
resource_dir = "/opt/darwin/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift"
```

These paths are illustrative. The descriptor's `environment` must equal the
selected provider's immutable image ID. Add SDK records such as `iphoneos`
for other destinations. Optional SDK `plugins` and `plugin_server` paths must
be configured together. Optional `env` entries describe worker library paths;
the Mac's environment is not inherited. Optional `programs` paths can select
`libtool`, `lipo`, or installed resource compatibility tools.

Run `once build --remote --compute workers Atlas` from the Mac project.
Independent compile actions use separate containers. Outputs return to the
Mac, and a subsequent build can restore them from cache without new workers.
The description and image selection both participate in cache identity.

This initial slice supports ordinary C/Swift libraries and applications with
ad-hoc signing. It does not make arbitrary Xcode projects Linux-compatible.
Swift macro compilation, SwiftPM build-tool plugins, explicit modules, Catalyst,
test runtimes, simulator execution, and production signing are not supported
by this cross-host path. Unsupported features fail rather than selecting Mac
tools silently. Resource processing requires compatible tools in the image.

## Current Boundaries

This backend demonstrates per-action dispatch, not a production fleet service.
It reuses an already available immutable image, but does not maintain a pool of
running workers. Each uncached action transfers a bounded archive; intermediate
outputs return through the coordinator before downstream actions receive them.
There is no worker-to-worker CAS transfer, cross-provider placement, or native
macOS worker support in Docker. Those require additional lifecycle, artifact,
and placement contracts rather than a different image setting.

See [Remote Execution](/guide/infrastructure/remote-execution) and the
[execution provider contract](/reference/infrastructure/execution) for the
shared behavior and integration requirements.
