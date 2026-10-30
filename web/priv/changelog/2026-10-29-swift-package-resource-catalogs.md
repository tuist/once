---
title: Swift package asset catalogs at runtime
date: 2026-10-29
---

Once now compiles processed Swift package asset catalogs into `Assets.car`, so
package images and colors resolve through `Bundle.module` at runtime. Multiple
catalogs in one target are compiled together without colliding on their
metadata files.

Native package discovery also recognizes implicit Apple resources, while
respecting explicit copy rules and excluded paths.
