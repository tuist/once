---
title: "Remotely executable scripts"
description: "Scripts that declare a contract can now run on remote compute through Once. The first providers are Microsandbox and Daytona, and the door is open for more."
date: 2026-05-27
author: "The Once team"
---

Working with coding harnesses concurrently makes you notice your machine in a way you had not before. You spin up two, three, sometimes ten agents on [git worktrees](https://git-scm.com/docs/git-worktree), each one running tests, builds, lints, code generators. At some point you hear the fans. Then you watch the timings stretch. Your laptop has a number of cores, and those cores have a clock speed, and no amount of parallelism wishes that away. The box on your desk is the bottleneck.

The natural reaction is to move some of that work somewhere else. Not because remote is intrinsically better, but because at that scale of parallelism the laptop cannot keep up. You want the agent to keep running where it runs, looking at the code where you look at the code, but the compile-and-test loop it spawns ten times in a row could happen somewhere with more cores and more bandwidth.

## Build systems got there first, but kept it to themselves

This is not a new idea. [Bazel](https://bazel.build) has supported remote execution for a long time. So has [Buck](https://buck2.build). The build system describes each action with explicit inputs and outputs, hashes them, and either fetches a cached result or ships the action off to a remote worker. The local machine watches results come back. It works, and it has worked for years.

The catch is that the capability has been locked inside the build system. If your action is a Bazel rule, you get remote execution. If your action is a script you wrote last week to regenerate a fixture or run a test suite, you do not. Most repositories have a lot more of the second kind than the first.

For a while there was no obvious way out of that, because remote execution was expensive to build and expensive to operate. You needed an action protocol, a worker fleet, a content-addressed cache, and the willingness to run all of that in production. Bazel had it because Google had it. Most teams did not.

That is changing. The AI wave has poured money and attention into compute. Sandboxes, microVMs, runner companies, content-addressed storage, all of it has become much cheaper, much more accessible, much more standardized. The infrastructure that used to require a Google-scale team to operate is something you can now stitch together from open primitives. The question stops being *can we do remote execution* and starts being *what should be allowed to use it*.

## The narrow waist

Once exists to sit at exactly that boundary. We have [written about this before](/blog/2026-05-14-hello-once/): Once wants to be the narrow waist between coding agents and the infrastructure underneath them. Above the waist, the things developers and agents actually do. Build code. Run tests. Generate files. Ship the script that has been part of the repository for years. Below the waist, the infrastructure that makes those things faster. Caches. Sandboxes. Remote workers. Whatever compute providers are willing to expose next.

The reason a narrow waist matters is that without one, every above-the-waist thing has to know about every below-the-waist thing. Your test runner has to know about your cache. Your script has to know about your sandbox provider. Your codegen tool has to know about your CI runner. Multiply that by ten agents running in parallel and the whole thing collapses under the integration matrix.

A narrow waist flips the shape. Anything that can declare what it needs and what it produces becomes an action. Any action can be cached. Any action can be placed somewhere. The agent does not negotiate with the provider. The script does not know which sandbox is running. Once handles the contract.

**Cacheable scripts were the first step. Remotely executable scripts are the second.**

## Placement is a user concern, not a repository concern

A Once script already declares its execution contract: inputs, outputs, environment, working directory. That contract is enough to make the script an action Once can reason about, and once the action exists, that same action can be placed on a remote provider without changing the script.

Notice what the contract leaves out. The script does not say *where* it should run. The repository describes the work. The person, or the agent, running it picks where, in the same way a Git repository does not pick which remote you push it through.

So placement lives at the edge of the invocation. A repository can name a provider in the script contract when the work is known to need one, but the person or agent running the command can also override it from the CLI:

```sh
once exec --remote --compute daytona -- ./scripts/test.sh
```

The Daytona adapter talks to the Daytona API directly and points at an existing sandbox. In development that is just environment:

```sh
export ONCE_DAYTONA_SANDBOX=my-sandbox
export ONCE_DAYTONA_API_KEY=...
export ONCE_DAYTONA_WORKDIR=/workspace/once
```

The first variable names the Daytona sandbox id or name. The second authenticates the API request, and can also be provided as `DAYTONA_API_KEY`. The third tells Once where the repository root lives inside that sandbox, and defaults to `/workspace` when you do not set it.

For checked-in scripts, the repository can still carry the default:

```sh
# once remote "daytona"
```

The script body does not change. The target does not grow provider-specific code. Once routes the action through the selected provider, streams stdout and stderr back as it runs, restores declared outputs into the workspace, and caches the result. A second run with the same inputs, on your machine or on a teammate's, replays from the cache without touching the provider at all.

The streaming detail matters. If output only arrives after the provider finishes, the mental model shifts to job submission, and that is not the interface developers are used to with scripts. We want the experience to stay boring. You run the script. Output appears. The process exits. The fact that the compute happened in a Daytona workspace is a property of where, not of how.

## Two providers, room for many

This first release ships with two providers.

[Microsandbox](https://github.com/superradcompany/microsandbox) is the one we use to exercise the whole flow end to end on a laptop. It is embedded in the Once binary rather than treated as another executable on the path. Once creates the microVM, mounts the repository, streams the process output, and tears the sandbox down when the command finishes. It does not give you more compute somewhere else, but it gives us a real provider contract that can run during development without asking people to stand up remote infrastructure first.

[Daytona](https://www.daytona.io) is the one we expect people to actually use. It gives you real remote workspaces with real horsepower, and it fits cleanly behind the same provider interface as Microsandbox. Point Once at the sandbox, pass `--remote --compute daytona`, and the compile-and-test loop your agents are spawning ten times in a row stops being a fight against your laptop.

The provider interface is intentionally small. We do not think the remote execution market should be a one-vendor story, and we want adding a new provider to be straightforward enough that anyone with a use case can do it. If you would like Once to support a provider we do not yet support, the contribution path is open. **PRs are welcome.**

## What we are aiming at

The real prize is not any single provider. It is the shape: a script that can describe itself, a contract that can travel, and an execution layer that can place actions where they belong without anyone above the waist having to think about it.

We will keep filling in the line. More providers. Smarter placement policies. Better observability for what is running where. The first contract is in. The interesting work begins now.
