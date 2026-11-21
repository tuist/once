---
title: Shared transitive inputs and compact build evidence
date: 2026-11-21
---

Target kinds can pass transitive sets to `run_action(inputs = ...)` and
`cmd_args` without copying their dependency closures into each declaration.
Struo retains shared references in analysis records, including response-file
contents. The built-in C family uses these sets for transitive build inputs.

Input digests compose shared nodes and include resolved file contents, so a
generated input changing at the same path still invalidates its consumers.
The new digest encoding invalidates older action-cache entries. Set structure
also contributes to cache identity, even when two declarations expand to the
same execution arguments.

Struo stores historical input fingerprints as shared components in a packed
database and expands them when you query evidence. Existing inline evidence
remains readable, and the evidence query format stays unchanged.

C and Zig link options contribute each library's ordered group once, with
shared dependencies after all their dependents so static archives resolve
correctly. Cached providers validate their complete referenced set graphs;
missing or damaged nodes trigger reanalysis and repair.
