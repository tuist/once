---
title: "Hosted test sharding through native Once commands"
date: 2026-10-26
---

Create a hosted CI matrix with `once query test-plan --shards` and run each
assignment with `once test --shard-plan --shard-index`. Once uses its existing
native Tuist authentication and configuration, without launching the Tuist CLI.

Hosted assignments preserve portable batch and fixture boundaries, including on
fresh workers. Each partition submits one immutable report with native
target/suite/name identities, ordered case retries, and explicit attempt and
revision counters. Empty assignments run nothing; missing evidence, failed
processes, and cancellation cannot produce a successful partition report.

This requires a release containing the hosted commands. Once 0.71.0 supports
portable plans only.
