---
title: Share the cache through an HTTP build cache
date: 2026-11-21
---

Struo can now share cached action results through any HTTP cache that stores
values with `GET`, `HEAD`, and `PUT`, such as a `WebDAV` build cache. Set
`kind = "http_cache"` with the server's `url`, and name the environment
variables that hold its token in `token_env`, or in `username_env` and
`password_env`. Credentials never live in `struo.toml` and are sent only over
HTTPS.

This works from any CI service whose jobs can reach the server. Struo does
not use CI services' own cache storage. Read the
[HTTP Cache guide](/docs/guide/infrastructure/http-cache) for setup, hosted
runner caches, the setups tested so far, and current limitations.
