# `dockerfile_image`

Builds an existing Dockerfile and exports a Docker or Open Container Initiative
image archive. Use it when the Dockerfile is the project contract, especially
when building from a base distribution or installing packages.

## Execution modes

`execution_mode` accepts `auto` (the default), `instructions`, or `buildkit`.
In automatic mode, Once translates supported instructions into independently
declared snapshot actions and falls back to BuildKit when required. Explicit
instruction mode rejects unsupported instructions instead of falling back.

Set `dockerfile` (default `Dockerfile`) and `context` (default `.`). Use
`format` to select `docker` or `oci`, `platform` for the destination, and
`target` to select a build stage. `build_args`, labels, annotations, and tags
configure the image. Network, builder, and layer-cache settings control the
BuildKit path. See the live schema for mode-specific attributes and defaults.

## Dependencies and outputs

The `images` role accepts `container_image` providers as named contexts;
`programs` accepts native `once_executable` providers. `caches` accepts
`dockerfile_image` targets that export declared layer caches.

The `build` capability exposes `archive`, `metadata`, `layer_cache`, `layout`,
and `plan`. The `lint` capability exposes `sarif` and normalized `results`.

```sh
once query schema dockerfile_image
once query example dockerfile_image dockerfile-image-instructions
```

Read [Container Images](/guide/graph/containers) before choosing a mode. It
explains prerequisites, instruction coverage, fallback behavior, and cache
policy. Use [`oci_image`](/reference/prelude/oci_image) instead when typed
layers can describe the complete image without a Dockerfile.
