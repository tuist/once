---
title: Explainable input fingerprints
date: 2026-07-30
---

Evidence records now explain why a build's inputs changed, not only that they
changed. Alongside the input digest it already tracked, Once records a
versioned input fingerprint that breaks the digest down into labeled
components for sources, generated inputs, dependencies, module code,
toolchains, command shape, and environment configuration.

Each component carries a category, a stable label, and its own digest, so two
runs can be compared to see exactly which part of the input moved. Targets that
run several actions aggregate their per-action fingerprints into a single
target-level view.

The fingerprint is available from both command and agent evidence queries, and
older evidence stays readable because the field is optional. Raw command
arguments, toolchain identities, and environment values are represented only by
their digests and are never stored in plain text, so the fingerprint stays safe
to share.
