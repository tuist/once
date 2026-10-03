# `oci_import`

Brings an existing Open Container Initiative image archive into the graph.
Use it when another tool already owns image production and Once should own
subsequent composition, loading, or publication.

## Input

The required `archive` attribute names a package-relative archive. Its bytes
are a tracked input, so replacing the archive rebuilds dependent targets.
Check that the exporting tool produced an OCI layout archive; an arbitrary tar
of a container filesystem is not an image archive.

## Outputs and composition

The target emits `container_image`. Its `build` capability exposes `layout`
and `archive`. Connect it to [`oci_image`](/reference/prelude/oci_image) as a
base, to [`oci_load`](/reference/prelude/oci_load) for local use, or to
[`oci_push`](/reference/prelude/oci_push) for publication.

```sh
once query schema oci_import
once query example oci_import oci-import
```

See [Container Images](/guide/graph/containers) for the import workflow.
