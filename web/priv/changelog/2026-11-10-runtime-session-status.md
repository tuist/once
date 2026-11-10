---
title: Reliable runtime session status reads
date: 2026-11-10
---

Runtime sessions now publish complete status records atomically. Polling
`once runtime status` while a session starts, stops, or exits no longer reads
an empty or partially written record. Runtime log queries and JSON-RPC session
inspection use the same complete records.
