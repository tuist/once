---
title: Cache owned Bazel actions
date: 2026-11-12
---

Once can cache supported Bazel action closures with declared inputs, outputs
and environments. Declare complete implicit host toolchain dependencies through
`toolchain_inputs` to enable spawn caching. Fresh spawns remove their declared
outputs before execution; cache hits restore them instead.

Command directory setup no longer captures existing sibling files as cache
artifacts. Only declared outputs are captured after the command runs. Directory
inputs assembled from generated files restore their cached children before
hashing, so cold builds and deleted-output restores publish the same complete
tree. Output parent directories are created only when actions execute, not
during cache probes.

File writes, materialized aliases and manifest-mapped runfiles use cacheable
Once actions. External executable snapshots preserve their permissions; older
snapshot declarations retain their default file-creation permissions. Owned
tests consume Bazel's expanded arguments once and use native shards and
environment, with additional runtime environment declared through `test_env`;
test execution remains uncached.

Required unsupported action contracts still delegate to Bazel and remain
uncached by Once. These changes do not provide hermetic remote execution or
cross-host cache portability.
