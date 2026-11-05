---
title: Run build actions on Tuist remote execution
date: 2026-11-05
---

`once build --remote` can now place graph command actions on a Tuist project's
remote execution workers. Once builds each action's input tree from its
declared inputs, uploads only the blobs the project cache is missing, runs the
action through the Remote Execution API, and installs validated outputs from
the same cache. A later build on any machine restores those results without
running the actions again.

Set `execution_image` on the Tuist infrastructure to the worker image the
project's execution service runs. Mismatched images, missing capacity, and
mismatched projects fail before any upload. A lost operation is submitted
again, and a broken stream reattaches to its operation, because the service may
run an action more than once.

Workers run `linux/amd64`, so commands must find their tools inside the worker
image. See [Tuist](/docs/guide/infrastructure/tuist#run-actions-on-tuist-workers).
