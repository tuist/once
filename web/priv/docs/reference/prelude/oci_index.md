# `oci_index`

Combines platform-specific images into one multi-platform Open Container
Initiative image index. Use it when consumers should select the appropriate
image for their operating system and architecture from one registry reference.

## Dependencies and metadata

The `images` role accepts `container_image` providers, one per platform.
Each image supplies its own operating system, architecture, and optional
variant. `annotations` records index metadata; `tag` and `tags` name the result.

## Outputs

The target emits `container_image`. Its `build` capability exposes `archive`,
`layout`, and `descriptor`. Connect the index to
[`oci_push`](/reference/prelude/oci_push) to publish the same manifest bytes.

```sh
once query schema oci_index
once query example oci_index oci-pull-and-extend
```

Read [Container Images](/guide/graph/containers) for the multi-platform workflow.
