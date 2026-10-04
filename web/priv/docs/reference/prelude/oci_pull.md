# `oci_pull`

Imports an image from a registry by content digest. It produces an Open
Container Initiative layout that other targets can extend, copy from, load,
or publish. The toolchain needs `crane`, not Docker.

## Pin the input

Set the required `image` registry reference and `digest`. Use `platform` to
select an operating system and architecture when the reference is an index.
`insecure` defaults to `false`; enable it only for a registry whose transport
requirements you understand.

A digest pin makes the base an explicit input. A mutable tag alone is not a
substitute: a registry can change its contents without a manifest edit.

## Outputs and composition

The target emits `container_image`. Its `build` capability exposes `layout`
and `archive`. Use it as the `base` of [`oci_image`](/reference/prelude/oci_image)
or as an `images` dependency of [`dockerfile_image`](/reference/prelude/dockerfile_image).

```sh
once query schema oci_pull
once query example oci_pull oci-pull-and-extend
```

See [Container Images](/guide/graph/containers) for authentication and composition.
