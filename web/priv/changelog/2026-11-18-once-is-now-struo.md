---
title: Once is now Struo
date: 2026-11-18
---

Once is now called Struo, Latin for "I build", and its home moves to
[struo.run](https://struo.run). Nothing about how builds work changes, but
every name you type does:

| Before | After |
| --- | --- |
| `once` command | `struo` |
| `once.toml` | `struo.toml` |
| `.once/` cache and output directory | `.struo/` |
| `# once` script headers and `#!/usr/bin/env -S once exec` shebangs | `# struo`, `struo exec` |
| `ONCE_*` environment variables | `STRUO_*` |
| `once_*` MCP tools | `struo_*` |
| `once` Rust crate, `Once` Swift module, `buildonce` npm and RubyGems packages | `struo`, `Struo`, `buildstruo` |
| `github:tuist/once` in mise | `github:tuist/struo` |

Rename `once.toml` files to `struo.toml`, `.once/` entries in ignore files to
`.struo/`, and update script headers, environment variables, and mise pins.
The first build after upgrading starts from an empty local cache, because
cache keys and the local cache directory now carry the new name. Sign in again
with `struo auth login`.

Live run reporting to Tuist keeps working against servers that have not been
updated yet.
