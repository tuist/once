---
title: More reliable Tuist cache uploads
date: 2026-10-03
---

Once now retries temporary failures when checking or uploading blobs and publishing action results to the Tuist cache. Normal HTTP/2 connection retirement no longer immediately abandons an upload or cache entry.

Retries are limited to three attempts. Interrupted uploads restart from the beginning with a fresh upload resource. Authentication failures, permission refusals, quota refusals, invalid requests, requests that exceed the request timeout, and local file errors still fail without retrying.
