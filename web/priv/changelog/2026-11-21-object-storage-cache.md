---
title: Share the cache through an object storage bucket
date: 2026-11-21
---

Struo can now share cached action results through an object storage bucket
you already run. Set `kind = "object_store"` with an `s3://`, `gs://`, or
`az://` URL, or a `file://` directory, and every machine keeps its local cache
while reusing results published by others. S3-compatible services such as
Cloudflare R2 and MinIO work through an `endpoint` option.

Credentials come from each service's usual environment variables and ambient
identity, never from `struo.toml`. For S3, Struo uses the AWS SDK credential
chain, so `AWS_PROFILE` or a `profile` option selects a `~/.aws` profile,
including SSO sessions and `credential_process` helpers. Downloads are
verified against their digests, outages fall back to local work, and
`read_only = true` or `STRUO_CACHE_READ_ONLY=1` stops untrusted jobs from publishing. Read the
[Object Storage Cache guide](/docs/guide/infrastructure/object-storage) for
setup, the services tested so far, and current limitations.
