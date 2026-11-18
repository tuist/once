---
title: Inspectable repeated-trial checks for builds and scripts
date: 2026-10-24
---

Use `struo build --verify-reproducible <target>` to compare two fresh builds,
including their dependency closure. Script checks with
`struo exec --verify-reproducible` now snapshot the whole workspace and rerun
`needs` dependencies and fingerprint commands inside each private trial. Both
trials start from the same snapshot at the same private path, with separate,
initially empty local Struo caches. Struo retains trial artifacts without
replacing outputs in your original workspace.

Execution status, declared artifacts, and captured logs are reported
separately. Two identical failures never pass. Artifact differences fail the
check; log-only differences are advisory when declared artifacts match. When
there are no declared artifacts, captured logs determine the comparison.

Every completed check returns an ID for `struo query verification <ID>`, also
available with `--format json`. Reports, per-trial results, decoded artifacts,
captured logs, and local trial caches remain under
`.struo/verifications/<ID>/`. File-level differences include bounded text
excerpts and first differing byte offsets when available, so you can inspect
what changed instead of comparing digests alone. These files remain local and
are not automatically published as project evidence.

Coding harnesses can request the same checks with `verify_reproducible: true`
on `struo_build_target` and `struo_exec_script`. The read-only
`struo_query_verification` tool retrieves a retained report without running
commands again. Script `--network deny` now also applies to `needs`
dependencies. Fingerprint commands that cannot enforce that policy fail
closed instead of silently running online.

A passing check means agreement under that artifact/log policy in two trials,
not proof of reproducibility. Struo does not perturb or control clocks, randomness,
or thread scheduling. Host toolchains, host tool caches, absolute paths, network
services, and external side effects are not isolated. The snapshot excludes
Struo runtime state and known declared script outputs, but other pre-existing
generated files remain. Only start a check when repeated execution of the
selected work and its dependencies is authorized.
