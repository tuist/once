# `oci_layer`

Packages native executables and source files into a deterministic Open Container
Initiative filesystem layer. Use it when you control the files in an image and
do not need a Dockerfile or a package-installation step.

## Inputs and configuration

Declare source files with `srcs`. The `programs` dependency role accepts
`once_executable` providers and places their executables in `program_dir`,
which defaults to `/usr/local/bin`. Source and runtime data use `data_dir`,
which defaults to `/app`.

Alternatively, set `archive` to a package-relative uncompressed tar layer.
`compress` accepts `none` (the default) or `gzip`. The `symlinks` map adds links
from container paths to their destinations. File modes, owner and group IDs,
and modification times are explicit so archive metadata does not depend on
the host. Set `os`, `architecture`, and `variant` when a prebuilt layer needs
to constrain the image platform.

## Composition and outputs

The target emits `oci_layer`. Its `build` capability exposes `blob` and `sha256`.
Connect it to the `layers` role of [`oci_image`](/reference/prelude/oci_image).
Layer order determines which files are visible in the final filesystem.

Inspect the complete contract and fetch a starter:

```sh
once query schema oci_layer
once query example oci_layer oci-image-minimal
```

Continue with [Container Images](/guide/graph/containers) for an executable,
layer, and image built together.
