---
title: Create and connect a project with struo connect
date: 2026-10-03
---

`struo connect` provisions a project with an infrastructure provider and writes
the binding into your repository. With Tuist as the provider it signs you in
when needed, creates the project, and records the account and project in the
root `struo.toml`, so a local project reaches a shared remote project in one
command. The workflow is provider neutral: it reuses the same provider
resolution as `struo auth login` and writes the same configuration the rest of
Struo reads.
