---
prev: false
next: false
---

# Container images

Build an existing Dockerfile with `dockerfile_image`. Once translates its
instructions into individual actions, each consuming and producing an image
snapshot. Cross-stage copies become explicit dependencies.
[BuildKit](https://docs.docker.com/build/buildkit/) executes the individual
instructions, including shell commands and package installation. A Dockerfile
that uses syntax Once cannot translate, such as heredocs, `ONBUILD`, or a
`# syntax=` frontend, builds as a single whole-file BuildKit action instead.
You do not have to choose.

To package a native executable built by Once without writing a Dockerfile, use
the `oci_layer` and `oci_image` target kinds, described below. Use
`dockerfile_image` when you already have a Dockerfile or need a distribution's
package manager. The two combine: a Dockerfile can use native executables and
digest-pinned images as build contexts, and a native image can extend a base
image.

## Build an existing Dockerfile

Once discovers a target named `image` in each directory containing a
`Dockerfile`. The Dockerfile and its neighboring files provide the build recipe
and context. With Docker and Buildx installed and running, build from the
directory containing the `Dockerfile`:

```sh
once build image
```

The Dockerfile's directory is the build context, including the files referenced
by `COPY`. From a parent workspace, use the relative package path. For example,
if the file is `server/Dockerfile`, run:

```sh
once build server/image
```

`once query targets` lists the discovered target names. Pass one of those names
to `once build`; a Dockerfile image is not selected by a bare `once build`.
Once uses the current Buildx builder when it supports image-layout exports. If
Docker's built-in builder is the implicit default, Once provisions and reuses a
dedicated container builder for the current Docker endpoint. This leaves your
selected builder unchanged. Set `BUILDX_BUILDER` to choose an existing
container or remote builder.

## Configure an image target

The discovered target uses the conventional `Dockerfile` and its directory as
the context. Add `once.toml` only when you need a different Dockerfile name,
context, platform, tag, or explicit Once action caching policy. Both paths are
relative to the package that contains the target:

```toml
[[target]]
name = "server_image"
kind = "dockerfile_image"

[target.attrs]
dockerfile = "server/Dockerfile"
context = "."
platform = "linux/arm64"
tag = "service:latest"
```

```sh
once build server_image
```

The default output is a Docker archive that you can pass to `docker image load`.
Set `format = "oci"` for an [Open Container Initiative image archive](https://github.com/opencontainers/image-spec).
Set `target` to export an intermediate Dockerfile stage instead of the final
image. Build arguments belong in `build_args`; do not put credentials there.

## Cache individual instructions

Enable `cacheable = true` with `pull = false` and an explicit `platform` when
you accept the cache contract for the instruction's remote inputs. Every base
image of a built stage must be pinned by digest (`name@sha256:...`); the build
fails otherwise, because a moving tag would keep serving a stale image from
the cache. A changed
source copied late in the recipe does not invalidate earlier instructions.
Once restores intermediate snapshots using its normal action cache. Literal
copy sources are tracked individually; wildcard sources, variable sources,
and context bind mounts conservatively track the full declared context.

Arguments retain their stage scope, and image snapshots carry environment,
working directory, and runtime configuration. Only stages needed for the
selected image are built. The final archive is exported by a separate action.
Intermediate snapshots preserve filesystem timestamps for tools that use them
to decide whether to regenerate files. BuildKit does not rewrite layers it
imports from a snapshot, so files inside the final archive keep the timestamps
from when their instruction ran. `source_date_epoch` still fixes the image
creation time and history. Set `reproducible_layers = true` when you need layer
bytes that are identical across independent rebuilds: each instruction then
rewrites the timestamps of its own layer to `source_date_epoch`, so later
instructions see normalized timestamps, which can change what timestamp-driven
tools such as `make` decide to rebuild. A cached result is stable until its
inputs change.
Mutable cache mounts remain on the BuildKit worker; snapshots contain the
resulting image filesystem, not those mount contents.

Files keep all of their permission bits, including setuid, setgid, and sticky,
when staged for a build step, and changing any of them invalidates the step.
The directories above a staged file keep their modes too. Changing only a
directory's mode does not invalidate a cached step, because a directory's mode
is not part of the digest of the files inside it; change a file or set the mode
in the Dockerfile with `COPY --chmod` or `RUN chmod` to force a rebuild.

Copy sources that are symbolic links also track their destinations, and empty
directories in the context are kept. A base image can store `ONBUILD` triggers
that run in its `FROM` step and need the whole context. Once asks the local
Docker engine, or the registry, whether a base declares any, and builds the
Dockerfile as one whole-file action when it does. If neither can be reached,
Once cannot know; use `execution_mode = "buildkit"` for such a base.

## Review inputs that can change

```sh
once lint server_image
```

Input checks report warnings with instruction locations and suggested repairs
for unpinned base images and `# syntax=` frontends, mutable package
repositories, downloaded pipelines, authentication mounts, and `COPY` sources
that are missing from the build context (often a sign that the Dockerfile
expects a parent directory as its context) whose contents do not participate in cache keys.
Agents can inspect the findings through the existing lint query surface.
The checks are advisory and do not alter cache eligibility. A repository
snapshot configured in a base image, for example, can make a package warning
a false positive. A clean report is not proof of reproducibility.

## Whole-file compatibility mode

The default `execution_mode = "auto"` builds instructions as separate actions
and switches to one whole-file BuildKit action when the Dockerfile uses
heredocs, `ONBUILD`, a `# syntax=` frontend, a custom escape character,
several variables in one `ARG`, or extended `${VAR:-default}` expansions. It
also switches when `export_cache`, `cache_to`, or a `caches` dependency is
set. `once lint image` reports a `whole_file_execution` note that names the
reason after a switch. Lint and queries work on every Dockerfile.

Set `execution_mode = "buildkit"` to always hand the complete Dockerfile to
BuildKit as one action, or `execution_mode = "instructions"` to fail with the
line number instead of switching.
Use this mode with Docker's built-in builder and `pull = false` when a base
image exists only in the local Docker image store. The automatically provisioned
container builder resolves base images independently and cannot see images held
only by that store. Otherwise, publish the base image to a registry and
reference it by digest.

Whole-file mode also supports portable BuildKit layer caches. BuildKit reuses matching layers even when Once must execute the build action.
Set `export_cache = true` to retain intermediate layers as a declared output.
Other image targets can import it with `[target.dependencies] caches = ["base_image"]`.
Once transfers that directory using the same artifact machinery as other target
outputs.

Enable `cacheable = true` with `pull = false` and an explicit `platform` only
for builds whose remote inputs are immutable. Once can then restore the
complete image and exported layer cache without running BuildKit. Pinning
base images alone is insufficient if `RUN` downloads changing packages.

For registry-backed layer sharing, use `cache_from` and `cache_to` lists of image
references. Registry exports retain all intermediate stages and require registry
credentials. Keep action caching disabled when publishing a registry cache.
Mutable cache-mount contents remain on the BuildKit worker and are not included
in exported layer caches.

## Execute on a remote worker

Provision a [remote BuildKit builder](https://docs.docker.com/build/builders/drivers/remote/),
then select its name with `builder`. Buildx sends the declared context to the
worker and returns the image artifacts to Once. Configure authentication and
memory limits on that worker. Once's local memory limit covers the client
process, not the worker's Dockerfile instructions.

See the [Starlark modules reference](/docs/reference/modules) for the input
inference, provider, cache dependency, and archive contracts.

## Import an existing image archive

`oci_import` brings an image exported by another tool into the graph. Point
`archive` at a `.tar`, `.tar.gz`, or `.tgz` file in image layout form, as
written by `docker save` (Docker 25 and later), `docker buildx build --output
type=oci`, or `crane pull --format oci`. The archive is a tracked input, so
replacing it rebuilds everything that depends on it. The image keeps the
names recorded in the archive. The result can be a
`base` for `oci_image`, loaded with `oci_load`, or published with `oci_push`.
Older `docker save` archives that carry only `manifest.json` are not layouts;
re-export them with a current Docker or `crane pull --format oci`.

```toml
[[target]]
name = "base"
kind = "oci_import"

[target.attrs]
archive = "images/base.oci.tar"
```

## Use other targets in a Dockerfile

Declare dependencies instead of copying files into the build context:

```toml
[[target]]
name = "base"
kind = "oci_pull"

[target.attrs]
image = "docker.io/library/alpine"
digest = "sha256:25109184c71bdad752c8312a8623239686a9a2071e8825f20acb8f2198c3f659"

[[target]]
name = "server_image"
kind = "dockerfile_image"

[target.dependencies]
images = ["base"]
programs = ["server"]

[target.attrs]
context_names = { base = "alpine:3.23" }
```

With this target, `FROM alpine:3.23` in the Dockerfile uses the pinned image
from the `base` target, and `COPY --from=server /server /usr/local/bin/server`
copies the executable built by the `server` target. A context is named after
its target unless `context_names` renames it. No registry is contacted, and
the dependencies count as pinned when `cacheable` is set.

## Native images

`oci_layer` packages executables and files into a deterministic layer, and
`oci_image` assembles layers into an image. Neither needs Docker.

```toml
[[target]]
name = "server_layer"
kind = "oci_layer"

[target.dependencies]
programs = ["server"]

[[target]]
name = "server_image"
kind = "oci_image"

[target.dependencies]
base = ["base"]
layers = ["server_layer"]

[target.attrs]
tag = "registry.example.com/team/server:latest"
tags = ["registry.example.com/team/server:v1"]
```

Set `annotations` on an `oci_layer` to record them on the layer's descriptor,
`compress = "gzip"` to store the layer as a deterministic gzip stream, and
`symlinks = { "/usr/bin/app" = "/usr/local/bin/app" }` to add links to it.

A `base` dependency makes the new layers extend an existing image, keeping its
layers, environment, and history. Setting `entrypoint` replaces the inherited
entry point and clears the inherited `cmd` unless you also set `cmd`, as a
Dockerfile `ENTRYPOINT` does. An empty list clears a value, and leaving an
attribute unset keeps the base's. A base for another platform than the image
is an error, and a base pulled with `platform = "all"` supplies the platform
the image asks for. Pin the base with `oci_pull`: the digest is required, so
the image cannot change under you, and Once caches the result.

## Multi-platform images

Build one image per platform and combine them:

```toml
[[target]]
name = "server_index"
kind = "oci_index"

[target.dependencies]
images = ["server_image_amd64", "server_image_arm64"]
```

Two images for the same platform are an error.

## Load and publish

```sh
once run server_load    # oci_load: docker load, or podman load; list several images to load a bundle
once run server_push    # oci_push: publish the image or index
```

`oci_push` sends the exact bytes Once built, so the registry digest equals the
digest of the local image or index. Set `sign = true` to sign that digest with
[cosign](https://docs.sigstore.dev/cosign/signing/overview/) after publishing,
with `cosign_key` naming a key file or key management URI, or empty for keyless
signing. `cosign_args` passes extra flags to `cosign sign`. `crane` performs the transfer and reads
credentials from your Docker configuration. Set `insecure = true` for a
registry without TLS. Install it with `mise use crane`.
