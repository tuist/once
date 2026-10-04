---
title: Safer cached output restoration
date: 2026-10-04
---

Once now stages cached files before replacing their workspace outputs. A concurrent reader keeps its complete original file instead of seeing it truncated during restoration. If a cached file cannot be read completely, the existing output stays intact.

File permissions are applied before replacement, and restoration continues to stream through bounded buffers rather than loading whole artifacts into memory.
