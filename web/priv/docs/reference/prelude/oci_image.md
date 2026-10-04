# `oci_image`

Assembles ordered filesystem layers into a Docker-compatible Open Container
Initiative image layout and archive. It does not need a running container
engine to build the image.

## Compose an image

The `layers` dependency role accepts `oci_layer` providers in base-to-top order.
The optional `base` role accepts a `container_image` provider with an image
layout. New layers extend that base rather than repackaging its filesystem.

Use `entrypoint` and `cmd` for the command, `env` for environment overrides,
and `user` and `working_dir` for the runtime context. The image also accepts
platform metadata, labels, annotations, exposed ports, volumes, and tags.
When extending a base, check its inherited configuration before overriding it.

## Outputs and next steps

The target emits `container_image`. Its `build` capability exposes `archive`,
`layout`, `descriptor`, `manifest`, and `config`.

Use [`oci_load`](/reference/prelude/oci_load) to load the image into a local
engine, [`oci_push`](/reference/prelude/oci_push) to publish it, or
[`oci_index`](/reference/prelude/oci_index) to combine platform-specific images.

```sh
once query schema oci_image
once query example oci_image oci-image-minimal
```

See [Container Images](/guide/graph/containers) for a complete manifest.
