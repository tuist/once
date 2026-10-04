---
title: "More resilient cache discovery and consistent analysis limits"
date: 2026-10-04
---

Once now reuses the last known remote cache endpoints when a discovery connection drops while receiving the response body. Without saved endpoints, the interruption becomes a cache miss instead of stopping the build. Malformed responses and authentication failures still surface as errors.

Host discovery commands also keep their output limits separate in the analysis cache. A command previously run with a larger limit can no longer bypass a later caller's smaller standard-error limit.
