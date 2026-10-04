---
title: Test results that follow the run
date: 2026-09-10
---

Swift Testing bundles now report every test and what actually became of it.
The testing library keeps a record of the run, and Once turns that record into
its normalized results instead of listing what the sources declare and
crediting each one with the run's exit status. A test that was filtered out,
skipped, or never reached is reported as such rather than as a pass, and an
issue a test marks as known no longer counts as a failure.

Where results still have to come from the XCTest host, that host reports the
run's outcome rather than each test's. Once lists the cases it finds so a shard
can address them, but lists them without a verdict, so a green run never
carries verdicts nothing produced.

Test bundles that depend on a Swift macro can now import it. Code behind
`canImport` of a macro module used to compile away, which quietly emptied the
test targets that exercise macro implementations. Such a bundle builds for the
host along with its dependencies, matching how a macro itself is built.
