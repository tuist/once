---
title: Create and connect a project with once connect
date: 2026-10-03
---

`once connect` provisions a project with an infrastructure provider and writes
the binding into your repository. With Tuist as the provider it signs you in
when needed, creates the project, and records the account and project in the
root `once.toml`, so a local project reaches a shared remote project in one
command. The workflow is provider neutral: it reuses the same provider
resolution as `once auth login` and writes the same configuration the rest of
Once reads.
