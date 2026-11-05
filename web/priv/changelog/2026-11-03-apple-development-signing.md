---
title: Development signing for Apple device builds
date: 2026-11-03
---

`apple_application` now accepts `signing = "development"` with a
`signing_identity` and `provisioning_profile`. Once embeds the profile, derives
the application identifier, team, and `get-task-allow` entitlements from it,
rejects entitlements the profile does not grant, and signs embedded frameworks
and resource bundles with the same identity before the application.

PKCS #12 and PEM identities sign with rcodesign, on Linux and macOS alike.
On macOS, a keychain identity name or fingerprint signs with `codesign`,
keyed by the resolved certificate fingerprint so renewals re-sign.
Identity files stay in place: they are never copied into the cache, and only
their digest enters cache keys.

Rebuilding an application into an existing output directory no longer seals
files an earlier build left behind, such as a removed resource or an old
provisioning profile, into its signature.

On Linux, `network = "deny"` now names the missing kernel feature, such as
seccomp filter mode, when the host cannot enforce it. Linux release binaries
are built against glibc 2.35 instead of 2.39.
