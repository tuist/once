---
title: "Reproducible test batches with stricter result validation"
date: 2026-10-09
---

Once can rerun one semantic test batch without changing its file or fixture
scope. Copy a batch ID from `once query test-plan --target <target>` and run
`once test <target> --test-batch <id>`. The same selector is available through
the test planning and execution MCP tools. Unknown or obsolete IDs fail before
execution and suggest current batches.

Automatic batches remain independent of worker count. Before scheduling a
complete target scope, Once checks that batches do not overlap and cover every
discovered unit. Exact results now reject duplicate or unrequested cases and
require passing evidence in isolated batch outputs. A successful process exit
alone cannot turn missing evidence or reported failures into a passing batch.

Custom test adapters that previously reported extra cases for exact requests
must honor the selected IDs. Keep setup diagnostics in runner metadata or
artifacts, and give parameterized cases distinct discovered IDs. Use target-level
batching when repeated startup or fixture setup outweighs parallelism.
