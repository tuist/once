# `oci_load`

Loads one or more image archives into a local container engine. This is a
runtime operation: invoke it with `once run`, not as a cacheable image build.

## Dependencies and engine

The `image` dependency role accepts `container_image` providers in load order.
Set `daemon` to `docker` (the default) or `podman`. The selected engine must be
installed and available locally.

The target exposes `build` and `run` with no output groups. Building its
prerequisites produces the archives; running it performs the engine load.
It does not start a container or publish an image.

```sh
once query schema oci_load
once query example oci_load oci-pull-and-extend
```

Use [`oci_push`](/reference/prelude/oci_push) for registry publication instead.
The [Container Images guide](/guide/graph/containers) connects both operations
to a built image.
