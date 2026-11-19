---
title: Tuist remote execution follows the service's worker image
date: 2026-11-19
---

`execution_image` is now optional for Tuist remote execution. Without it,
`struo build --remote` asks the execution service which worker image it runs
and uses that, so a worker image update no longer breaks builds until you
repin. Pin `execution_image` only when you want builds to fail instead of
following an image change. Remote Elixir toolchain descriptions no longer need
an `environment` either.

This release also brings the rest of Struo's Tuist remote execution client
work: image-bound Elixir toolchains, client-facing cache discovery, phase
timings in the logs, batched output downloads, safe restoration of mixed and
linked outputs, and running at the account's advertised concurrency in VMs
sized to each action.
