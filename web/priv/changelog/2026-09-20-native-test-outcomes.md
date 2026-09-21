---
title: Native Apple test outcomes in Once reports
date: 2026-09-20
---

Apple test bundles run by the XCTest host now translate completed test cases
from the native runner output into Once results, including their outcome and
duration. This also covers Swift Testing cases in a bundle that uses both
runners. Source discovery still records cases that the runner did not reach,
but leaves their outcome unknown instead of treating a passing process as
proof that every discovered case passed.
