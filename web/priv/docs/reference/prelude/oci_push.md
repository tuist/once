# `oci_push`

Publishes a built image or multi-platform index to a registry while preserving
its manifest digest. Invoke it with `once run`: publication is a side effect,
not a cached build result. The toolchain needs `crane`, not Docker.

## Destination and authentication

The `image` role accepts a `container_image` provider. Set the required
`repository` to the destination and `remote_tags` to the tags to update; the
default is `["latest"]`. Authenticate with the registry before running the
target. Avoid putting credentials in a manifest.

`insecure` defaults to `false`. Optional signing uses `sign`, `cosign_args`,
and `cosign_key`; a signing workflow also needs `cosign` and its credentials.

The target exposes `build` and `run` with no output groups. Build constructs
the dependency closure; run uploads the layout and updates the requested tags.

```sh
once query schema oci_push
once query example oci_push oci-pull-and-extend
```

See [Container Images](/guide/graph/containers) for the image-to-registry workflow.
