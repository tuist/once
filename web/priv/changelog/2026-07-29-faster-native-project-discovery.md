---
title: Faster native project discovery
date: 2026-07-29
---

Once now discovers native Cargo, Mix, and future ecosystem projects with one
shared filesystem pass. Workspace include and exclude patterns prune unrelated
directories before Once enters them, reducing startup work in large
repositories without changing the targets that discovery produces.

Discovery also keeps its own memory usage predictable. Project kinds that own
nested packages retain only their shallowest matching roots, while kinds that
recognize every nested project have a 16 mebibyte retained-result limit. Once
stops with guidance for narrowing the workspace if that limit is reached,
instead of allowing the number of matching directories to cause unbounded
memory growth.
