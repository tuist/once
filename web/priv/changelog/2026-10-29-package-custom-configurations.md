---
title: "Consistent package settings in custom Xcode configurations"
date: 2026-10-29
---

Swift packages imported from Xcode now follow Swift Build's configuration
mapping: names containing `debug` or `development`, ignoring case, select Debug;
all other names select Release. Native project optimization settings and
compilation conditions do not change that mapping.

The automatic `DEBUG` definition and conditional Swift and C-family package
settings now agree, including macro targets and their host dependencies.
For example, `Development` enables debug-only package settings, while `Staging`
selects release-only settings.
