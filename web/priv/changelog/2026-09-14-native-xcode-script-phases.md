---
title: Run native Xcode script phases with reconciled build settings
date: 2026-09-14
---

Once now retains Xcode script phases without source or archive outputs,
including validation and product-processing scripts. Preparation runs before
compilation, resource generators can run between linking and packaging, and
product scripts run after assembly and before final signing. Later scripts
consume the bundle produced by earlier scripts, including undeclared resources
created by always-run phases. Deleted resources stay deleted, and restoring a
captured bundle no longer overwrites newer outputs inside it.

Scripts receive native build settings mapped to Once's workspace, product,
and intermediate directories, including input and output file-list variables.
Fully declared scripts can reuse cached results. Always-run scripts and
scripts with untracked inputs bypass the cache while allowing individual
compilation actions to remain cached. Installation-only phases remain excluded
from ordinary builds.

Native shell phases execute in the workspace so absolute build-setting paths
and in-place edits address the same products. Caching relies on complete input
and output declarations, not filesystem isolation.
