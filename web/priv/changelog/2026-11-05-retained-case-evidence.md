---
title: Inspect retained test cases and producer attempts
date: 2026-11-05
---

`once query test-results` and the read-only `once_query_test_results` MCP tool
now accept an exact retained case id and an optional one-based producer attempt
position. Without a position, Once selects the last attempt in the retained
array. Full reports and summary queries are unchanged.

The focused response keeps target, case, and attempt statuses distinct,
preserves producer attempt fields, and identifies the retained file bytes with
a digest. Artifact paths remain unchecked target metadata. Inspection never
evaluates the current graph or refreshes discovery, and its rerun suggestion
applies only to the whole target under the current configuration.

Focused reads have explicit file and logical projection limits with structured
repair diagnostics. See [retained case evidence](/docs/guide/graph/testing#inspect-retained-case-evidence)
for the limits and CLI/MCP contract.
