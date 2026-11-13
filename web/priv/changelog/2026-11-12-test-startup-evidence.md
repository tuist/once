---
title: Test startup failures retain fresh evidence
date: 2026-11-12
---

Scheduled whole-target tests no longer reuse results from an earlier run when
dependency resolution or another startup step fails before the test runner
starts. The retained normalized failure includes a bounded startup diagnostic,
so missing dependencies are visible when inspecting test results later.

Nested Mix projects remain independently discovered test suites. Fetch their
locked dependencies and provision required services before running them;
native test aliases, including database setup, are preserved.
