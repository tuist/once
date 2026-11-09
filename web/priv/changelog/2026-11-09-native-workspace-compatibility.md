---
title: More reliable Nx tasks and Bazel tests
date: 2026-11-09
---

Nx task filters now retain transitive prerequisites, such as a build's
`compile` task, while ignoring prerequisites that the project does not
define. Commands find installed package tools such as `tsc` through the
nearest `node_modules/.bin` directory. Declared outputs are excluded from
project source inputs, so deleting generated files does not change the
cache key and those files can be restored from the Once cache.

Nx prerequisite discovery also crosses projects without the requested task.
Package scripts run from their project directory, and command executors
preserve forwarded arguments such as a production build mode. Optional
outputs, including coverage directories, are cached only when present;
ordinary Once action outputs remain required. Generated Vite timing caches
no longer cause unchanged tests to miss the cache.

Bazel tests with `TestRunner` actions now use Bazel's native test strategy
instead of invoking an incomplete test command from the action graph.
This preserves Bazel's test environment, runfiles setup, and failed-test
status. Bazel owns cache reuse for delegated commands; these are not Once
action-cache hits.
