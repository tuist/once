---
title: Reliable home directories for test runners
date: 2026-11-09
---

Python, Ruby, and JavaScript test targets now receive an absolute private home
directory at execution time. This fixes tools such as SimpleCov that expand
`~` and reject relative home paths, while keeping action identities independent
of the checkout location. Explicit environment overrides continue to work.
