---
title: Remove obsolete objects from C and C++ archives
date: 2026-11-09
---

Removing a source from a `c_library` target now removes its object from the
rebuilt static archive. Uncached archive actions recreate the library from the
current object set instead of retaining members from an earlier build.

Cached builds continue to restore the declared archive and object outputs.
