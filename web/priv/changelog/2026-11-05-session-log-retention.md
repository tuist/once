---
title: Bound session log retention
date: 2026-11-05
---

Once now prunes its per-invocation session logs automatically. Each command
keeps the newest 100 logs, up to 500 megabytes in total, and deletes older
ones. The three newest logs and the current session are always kept, so one
large session cannot erase the history around it.

Set `ONCE_LOG_RETAINED_SESSIONS` or `ONCE_LOG_RETAINED_BYTES` to change either
bound. A directory that accumulated logs before this change drains over the
next few commands rather than stalling the first one.
