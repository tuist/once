---
title: Edits to linked workspace packages rebuild their dependents
date: 2026-11-19
---

In JavaScript workspaces, package managers link each workspace package into
`node_modules`. Struo's change tracker followed those links while watching the
workspace and could then report an edit to a package's sources under the
linked path instead of the real one. A rebuild after the edit could reuse the
previous result. The tracker now reports every change under its real path,
and inputs that live behind a linked directory are always checked on disk.
