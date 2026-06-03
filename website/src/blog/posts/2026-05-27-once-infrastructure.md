---
title: "Once infrastructure"
description: "Once now has an infrastructure layer that lets the local script runtime talk to external providers, starting with Tuist as a shared cache for scripts."
date: 2026-05-27
author: "The Once team"
---

We started Once with two ideas in mind: **skipping work** and **distributing work**. If a machine has already produced a result from a set of inputs, another machine should not have to repeat it. And if a piece of work can run somewhere better suited for it, like CI, a remote agent, or a faster machine close to the cache, the local workflow should be able to benefit from that without changing how the work is described. The boundary between laptop, CI, and agents is artificial. The inputs did not change. The action did not change. The fact that the work happened somewhere else should not force everyone to pay for it again.

Once already models work as actions with explicit inputs, outputs, environment, working directory, and resource requirements. That gives us a content-addressed view of a task. If the action and its inputs are the same, the result should be reusable. Until now, that reuse lived in the local CAS. Useful, but still local. The next step was obvious: the cache should be able to live behind something else.

So we added the concept of infrastructure to Once. The word matters because it says what this layer is trying to be. Not a feature hidden inside one command. Not a special integration baked into the runner. A small boundary where Once can talk to external providers while keeping the core model stable. The script runtime should not know if the cache is a directory on disk, a Tuist cache node nearby, or another provider we add later. It should ask for blobs and action results through the same interface and let the configured infrastructure decide where those bytes come from.

The first place where this shows up is caching. Once now has a cache infrastructure abstraction, with the local CAS as the default and [Tuist](https://tuist.dev) as one of the first remote infrastructures. The local cache still sits in front, so fast local hits stay fast, but successful actions can be mirrored to Tuist and later restored from another machine. We are not making Once depend on Tuist as a hardcoded backend. We are giving Once a way to connect to infrastructure, and Tuist happens to be the first one we care deeply about because it is where we are building low-latency caching for the developer workflows we know well.

The configuration is intentionally small. If your workspace wants to use Tuist directly, you can put the cache infrastructure in the root `once.toml`:

```toml
[infrastructure.cache]
kind = "tuist"
account = "acme"
```

By default Once talks to `https://tuist.dev`. If you self-host, you can make that explicit:

```toml
[infrastructure.cache]
kind = "tuist"
url = "https://tuist.dev"
account = "acme"
```

There is also a named-provider form, which is the one we expect to age better for teams. A repository can say which provider it wants, and each developer or machine can define that provider once in the user config. That keeps repository configuration focused on the project scope, while the details of the infrastructure live at the system level:

```toml
# once.toml
[infrastructure.cache]
name = "tuist"
account = "acme"
```

```toml
# ~/.config/once/config.toml
[infrastructures.tuist]
kind = "tuist"
url = "https://tuist.dev"
```

Authentication follows the same idea. It should be a system concern, not something every repository invents again. On a developer machine, Once can authenticate once and reuse that session through the system credentials directory:

```sh
once auth login --provider tuist
```

That login is not tied to one checkout. It is tied to the provider identity and stored under the user's Once configuration area. The point is the same as with Git credentials or cloud CLIs. You should not have to teach every project how to authenticate with the same service. You authenticate with the service once, and then every workspace that resolves to that infrastructure can use it. If a repository says `name = "tuist"` and your user config points that name to Tuist, the build command does not need another flag. It resolves the infrastructure, loads the stored session, and goes through the same cache interface as the local provider.

Where this gets more interesting is scripts. Scripts are one of those humble pieces of infrastructure that every project has, but that almost no build system treats well. They are usually invisible to caching because they live outside the formal graph. Once has been moving in a different direction: a script can describe its execution contract in the file itself, and once that contract is explicit, the script becomes cacheable like anything else.

For example, imagine a small script that turns an input file into an output file:

```sh
#!/usr/bin/env -S once exec -- bash
# once input "../input.txt"
# once output "../out.txt"
# once cwd ".."

tr '[:lower:]' '[:upper:]' < input.txt > out.txt
cat out.txt
```

You can commit that as `scripts/uppercase.sh`, make it executable, and run it directly:

```sh
chmod +x scripts/uppercase.sh
./scripts/uppercase.sh
```

The first run is a miss. Once sees the script path, the declared input, the working directory, the command line, and the declared output. It runs the script, stores stdout, stderr, and `out.txt`, and publishes the action result through the configured cache infrastructure. If the workspace is configured with Tuist, the local CAS gets the result immediately, and Tuist can receive it for other machines. The second run on your machine becomes a local hit. A teammate or an agent running in another environment can get a Tuist hit and restore the output without rerunning the script.

You can also make the script itself executable through Once:

```sh
#!/usr/bin/env -S once exec -- bash
# once input "../input.txt"
# once output "../out.txt"
# once cwd ".."
```

This is a small example, but it points at the shape of the system. A lot of developer automation starts as a script because scripts are easy to write and easy to understand. The problem is that they often remain opaque forever. By letting scripts declare their inputs and outputs, Once gives them enough structure to participate in the cache without forcing every team to rewrite them as a plugin, a rule, or a custom task type. And by letting the cache provider be Tuist, the result is not trapped on one laptop.

There is a larger thread here that connects to what we are building at [Tuist](https://tuist.dev). Compute is becoming more fluid. Your build might run locally, in [GitHub Actions](https://github.com/features/actions), in [Codex](https://openai.com/codex/), in a [remote VM](https://www.coder.com/), or somewhere we are not thinking about yet. If every environment starts from zero, we lose. If the useful work can travel through a shared cache, we get a system where the location of compute matters less and the correctness of the action model matters more.

Once infrastructure is a step in that direction. The local model stays simple. The external provider stays swappable. Authentication is shared at the system level. Scripts get to become first-class cached actions without losing their simplicity. It is not the whole story, but it is the kind of foundation we like: small enough to understand, explicit enough to trust, and open enough that the next provider does not require changing how people describe their work.
