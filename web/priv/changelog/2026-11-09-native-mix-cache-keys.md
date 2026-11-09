---
title: Improve native Mix test discovery and dependency caching
date: 2026-11-09
---

Native Mix projects now report their test targets explicitly, so `once test`
also selects tests in projects without shared development and test dependencies.

Mix dependencies expose bytecode, private resources, and headers as separately
fingerprinted outputs while retaining their complete native application
directories. Compiler-manifest timestamps no longer change consumer cache keys
when the consumed outputs are unchanged.
