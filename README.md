<p align="center">
  <img src="assets/logo.png" width="120" alt="Struo logo" />
</p>

<h1 align="center">Struo</h1>

<p align="center">
  <strong>A modern build system for the age of agents.</strong><br />
  Caching and remote execution for developers and coding agents alike.
</p>

<p align="center">
  <a href="https://github.com/tuist/struo/actions/workflows/struo.yml"><img src="https://github.com/tuist/struo/actions/workflows/struo.yml/badge.svg" alt="CI status" /></a>
  <a href="https://github.com/tuist/struo/releases"><img src="https://img.shields.io/github/v/release/tuist/struo?label=release" alt="Latest release" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license" /></a>
</p>

<p align="center">
  <a href="https://struo.run/docs"><b>Documentation</b></a> ·
  <a href="https://struo.run/docs/guide/getting-started"><b>Getting started</b></a> ·
  <a href="https://struo.run/blog"><b>Blog</b></a> ·
  <a href="https://struo.run/changelog"><b>Changelog</b></a>
</p>

---

*Struo* is Latin for "I build". Bring caching and remote execution to your
builds, wherever you start:

| Start from | What Struo does |
| --- | --- |
| **Your scripts** | Makes existing automation cacheable and remotely executable with a few `# struo` header lines. |
| **Your projects** | Builds Cargo, Swift Package Manager, Xcode, Bazel, and other existing projects without migrating to a new build definition. |
| **Your build graph** | Declares typed targets in Struo's own language and composes them into one queryable graph. |

Developer experience comes first. Reliable, reproducible builds should fit your
project, not require you to adopt the assumptions of an enterprise-scale build
system.

## Quick start

Install Struo with [mise](https://mise.jdx.dev/):

```sh
mise use -g "github:tuist/struo@$(mise latest github:tuist/struo)"
```

Point it at a project you already have:

```sh
cd path/to/project
struo query targets
struo build
struo test
```

Or make a script cacheable by declaring what it reads and writes:

```sh
#!/usr/bin/env -S struo exec -- bash
# struo input "message.txt"
# struo output "build/greeting.txt"
mkdir -p build
cp message.txt build/greeting.txt
```

The first run records the result; the next one restores `build/greeting.txt`
from the cache until `message.txt` changes.

## Built for agents

Every capability is reachable without reading prose docs. Agents discover
target kinds, pull runnable starters, validate drafts, and commit edits through
the `struo mcp` server, and every MCP tool has a matching `struo query` or
`struo edit` command so humans can reproduce what an agent did.

## Learn more

- [Documentation](https://struo.run/docs): guides for every ecosystem and the full reference.
- [SDKs](https://struo.run/docs/guide/sdk/): cache access from Rust, Swift, JavaScript, and Ruby.
- [RFCs](rfcs/): design documents for the build graph and runtime.

Struo was previously called Once.

## License

Struo is released under the [MIT license](LICENSE).
