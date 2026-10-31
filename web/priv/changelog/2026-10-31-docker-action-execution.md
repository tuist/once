---
title: Dispatch build actions through Docker
date: 2026-10-31
---

`once build --remote --compute <provider>` can dispatch declared graph command
actions through an execution provider while Once retains dependency scheduling
and caching. The new Docker provider makes that boundary testable locally:
each uncached command receives its declared inputs in a fresh container, and
validated outputs return to the coordinator for downstream actions.

Docker requires an already prepared immutable image ID. Worker CPU and memory
limits follow both coordinator and actual Docker daemon admission budgets, and
action containers receive no host workspace or Docker socket mount. Target
kinds can declare command resource requests through `run_action`. Selecting
remote execution partitions build configuration and cached results; unsupported
requirements fail rather than silently falling back to a local command.

Remote transfers reject symlink chains that escape declared trees and refuse
symlinked parent traversal on the coordinator. Ordered graph dependencies,
response files in cleaned scratch directories, and hard-linked outputs retain
their expected behavior through Docker execution.
Secure archive-backed remote transfer currently supports Linux and Apple
coordinators; other platforms fail closed with an explicit unsupported error.

An image-bound Apple toolchain description lets a Mac resolve an original
Xcode project and send C/Swift compilation inputs and dependency outputs to
Linux workers without exporting the graph. Linux-compatible compilers, Darwin
SDK resources, and ad-hoc signing tools must already be installed in the image.
This does not make arbitrary Xcode projects or Mac-only build tools portable.

Failed cleanup retains host and worker reservations, including exclusive slots,
until reconciliation confirms deletion. Docker archive transfers now honor
progress-stall and post-upload acknowledgement deadlines as well as the total
artifact-transfer deadline.

The initial backend does not maintain running worker pools or transfer
intermediates directly between workers. See
[Docker execution](/docs/guide/infrastructure/docker) for setup and limits.

Linux Apple builds also fingerprint complete SDK bundles so valid internal
cross-directory links remain usable, and select the SDK's Darwin compiler
runtime for Swift's Clang link step instead of the Linux host runtime.
