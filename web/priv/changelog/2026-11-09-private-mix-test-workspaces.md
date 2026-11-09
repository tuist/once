---
title: Opt into private native Mix test workspaces
date: 2026-11-09
---

Mix test targets can now opt into a fresh private workspace with the native
`_build/test/lib` layout. Cwd-relative test writes and application resource
lookup use the same copied application trees, without changing developer build
outputs. Native compiler metadata is retained.

Set `staged_workspace = true` on an `elixir_test` target, or
`test_staged_workspace = true` on an explicit `mix_workspace` seed. Only declared
inputs are staged. The default original-package cwd is unchanged. The opt-in
changes runtime cwd, not compiled source locations, and rejects symbolic links
and shell setup. See the [test target reference](/docs/reference/prelude/elixir_test)
for details.
