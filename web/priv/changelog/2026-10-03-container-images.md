---
title: Container images without Docker or scripts
date: 2026-10-03
---

Native container images are assembled by Once itself. `oci_image` can extend a
base image, name several tags, and combine with other platforms through the new
`oci_index`. `oci_pull` fetches a base by content digest, so it is immutable
and cached. `oci_push` publishes an image or multi-platform index with the same
digest Once built, and can sign it with cosign. `oci_import` brings in an image archive exported elsewhere. `oci_load` loads an archive
into Docker or Podman. Layers can be gzip-compressed and carry symbolic links.

Dockerfile images no longer need Python. Each instruction is a single Docker
build step, and `ARG` and `ENV` values are resolved up front. A Dockerfile can
use native executables and pinned images as named build contexts, instead of
copying files into its context.

Two correctness fixes reach beyond containers. Changing only a file's
permission bits, such as making a script executable, now invalidates cached
results built from it. Copied directory trees keep their empty directories.
