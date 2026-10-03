# Request for Comments 0012: Container registry, index, and base-image targets

## Decision

Container workflows should compose from typed targets that share one
`container_image` provider, so a Dockerfile image, a native image, a pulled
base, and a multi-platform index are interchangeable where an image is
expected. Rust stays generic: the targets use ordinary declared actions, one
deferred planner, and two small host primitives, and external registries are
reached through an existing client rather than reimplemented.

## Targets

`oci_pull` fetches one image, or every platform of an index, by content digest
into an image layout. The digest is required, so the result is immutable and
the action is cacheable. `oci_image` accepts a `base` dependency and appends
its layers, configuration, and history to the base's. `oci_index` combines
single-platform images and rejects duplicate platforms. `oci_push` publishes
an image or index, and `oci_load` loads an archive into a local engine. A
`dockerfile_image` consumes images and native executables as named build
contexts through the `images` and `programs` roles.

## Why a registry client and not BuildKit

BuildKit can push a layout, but it re-composes manifests it did not produce:
an image assembled by Once was republished under a different manifest digest,
and an index under a different index digest. Publishing must preserve the
digest Once computed, because the digest is how deployments pin an image. A
registry client that copies the layout verbatim preserves it, so pull and push
use `crane` and the Docker credential configuration. Neither needs Docker.

## Image assembly

An image layout depends on digests and sizes that exist only after the layer
archives are built. `oci_image` and `oci_index` therefore declare a deferred
planner. After the layers finish, the planner reads their digests and sizes,
reads the base image's manifest and configuration, and declares ordinary
actions that write the configuration, manifest, index, and content-addressed
blobs. A layout directory is never deleted. Every file in it is content addressed or
identical on every rebuild, so concurrent builders in one workspace only write
the same bytes, and the archive lists exactly the files that belong to the
image, so blobs left by earlier builds do not leak into it.
The only new host primitive is `host_file_size`; `host_command` gains an
opt-in `check = False` for idempotent setup that a later probe verifies.

## Publishing

`crane` pushes a layout that holds exactly one entry, while a container engine
needs one entry per image name to import several names. `oci_push` therefore
plans its work after the image is built: it reads the layout, writes a view
with a single unnamed entry whose `blobs` directory links to the image's, and
pushes that view. The remaining tags and the optional signature refer to the
digest it just published, `repository@digest`, never to a tag another writer
could move. A layout that holds different images is rejected.

Tags are validated before anything is published, because a value that looks
like an option would otherwise be read by the client as one and could leave a
partial publication behind.

## Bases and platforms

A base may be a single image, an index, or an index of indexes, as `oci_pull`
produces with `platform = "all"`. The planner resolves nested indexes to the
requested platform and rejects a base whose configuration is for another
operating system, architecture, or variant, because relabelling it would
produce an image whose executables cannot run. `entrypoint` and `cmd` follow
Dockerfile semantics: setting an entry point resets an inherited command, an
empty list clears a value, and leaving a value unset keeps the base's.

Sizes in descriptors are byte counts, so they are computed from the UTF-8
length of the document, and the shared JSON encoder escapes control characters.

Dockerfile builds pass a named image to BuildKit as `oci-layout://dir:tag`,
using the tag of the image's first name, because a layout with several names
and no matching tag is ambiguous to BuildKit.

## Naming

Each image name becomes one layout entry whose `io.containerd.image.name` is
the fully qualified reference and whose `org.opencontainers.image.ref.name` is
its tag. That is the form container engines import without creating duplicate
names. BuildKit resolves a single-image layout without a tag.

## Layers and signing

`write_archive` gained a gzip format with a fixed header, so a compressed layer
is identical on every machine, and it reports both the compressed digest (the
blob) and the uncompressed one (the diff identifier the image configuration
needs). It also gained symbolic-link entries. `oci_push` can sign what it
pushed: after the registry reports the digest, a deferred planner reads it and
declares the cosign action for exactly that digest.
