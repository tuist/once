---
title: "Portable test batches for hosted CI sharding"
date: 2026-10-25
---

Every `struo query test-plan --format json` export is portable and replayable.
Fresh CI workers can execute assigned opaque batches with `struo test
--plan plan.json --batch <id>` without transferring private discovery caches or
reconstructing runner filters. Matching checkouts and tool versions can live at
different paths. Changed discovery inputs or configuration fail validation;
file fixtures and exact-result checks remain intact.

Portable execution returns all assigned batch outcomes, including failed tests,
with explicit completeness and success fields. External coordinators can attach
a test-run group and partition index to authenticated native diagnostics and
submit one consolidated test report per CI partition without duplicating each
batch's cases.
