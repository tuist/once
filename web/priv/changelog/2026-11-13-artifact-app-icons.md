---
title: Application icons and Android labels in published artifacts
date: 2026-11-13
---

Exported `apple_application` and `android_binary` artifacts now carry the
application's own icon as an `icon` PNG payload, named through
`spec.icon_payload` on their install interface. Android metadata also records
`display_name` from the APK's default application label, so previews show the
app's name instead of its application ID. Apple applications use the icon their
bundle declares, or the source images of their `app_icon` asset catalog set.
Applications without a usable icon publish exactly as before.

Artifact providers can declare a payload as `optional`. When its output is
empty, the payload and the interface `spec` entries ending in `_payload` that
name it are left out of the exported artifact.
