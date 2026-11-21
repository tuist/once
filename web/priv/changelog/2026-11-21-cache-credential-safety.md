---
title: Safer cache credentials and cancelled uploads
date: 2026-11-21
---

Remote cache uploads retain their resource reservations until the backend
finishes, even when the calling action is cancelled. This keeps directory
stores and other backends within their transfer budgets while writes finish.

Object storage errors no longer include raw provider diagnostics or Azure
CLI credential-helper output. Rejected Azure and Google OAuth token exchanges
fail the command instead of falling back to local execution; unreachable or
throttled credential services still allow local work.
