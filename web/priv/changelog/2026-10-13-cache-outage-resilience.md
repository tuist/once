---
title: "Keep building through transient cache outages"
date: 2026-10-13
---

Struo can now continue executing scripts and build actions when the remote cache
remains unavailable after bounded retries. Fresh results stay in the local cache
and can be reused by later runs while the remote service recovers.

If a deferred output becomes unavailable, a build can run its recorded producer
again to reconstruct the exact expected bytes. Existing resource admission,
repair limits, and digest verification still apply.

Outages are logged and remain distinct from confirmed missing content. Cache
reuse and restoration do not hide credential refusals, corruption, malformed
cache records, or local filesystem failures. Explicit cache and SDK reads keep their existing error semantics.
