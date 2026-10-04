---
title: Preserve Swift dependency traits
date: 2026-09-09
---

Native Swift package builds now preserve traits requested by dependencies.
Once combines requests across the package graph and expands traits that enable
other traits before compiling libraries and macros. This fixes missing types
when a dependency exposes functionality behind an opt-in trait.

Explicit trait lists, disabled defaults, and conditional trait requests follow
the package declarations. The same resolved traits control compiler settings
and optional target dependencies.
