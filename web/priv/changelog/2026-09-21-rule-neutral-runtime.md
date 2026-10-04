---
title: Rule-neutral build and test reporting
category: Added
---

Once now keeps build and test reporting independent from target-kind names. Native compatibility commands select declared capabilities, tools, and resolver metadata, while test runners communicate through the normalized test-results contract. Toolchain names remain opaque in shared argument telemetry unless a workspace explicitly marks a value safe.
