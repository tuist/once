# Why Once

You should not have to rebuild the same inputs just because the work moved
from your laptop to continuous integration, or from one coding agent to
another. Once makes that work reusable without asking you to replace every
working tool in your repository first.

The idea is simple: describe what an action reads, what it writes, and the
environment it needs. Once gives that contract a content-based identity and
stores the result. When the contract is unchanged, it restores the result
instead of repeating the work.

Build systems such as Bazel have demonstrated the value of explicit
dependencies, shared caches, and remote execution. Once brings that model to
existing repository automation through native project discovery, annotated
scripts, and typed targets. The migration can start with one useful action,
not a repository-wide rewrite.

## Agents Change The Shape

Coding agents push against that model. They can run concurrently across
worktrees, explore several fixes at once, retry tests, regenerate outputs,
and ask for the same expensive setup from different sessions. Work that was
annoying when one developer repeated it becomes wasteful when many agents
repeat it in parallel.

That changes what feels worth optimizing. The capabilities once associated
with large enterprise build systems start to matter in ordinary
repositories: stable action identities, deterministic inputs, reusable
outputs, shared cache storage, and the option to run work somewhere other
than the current laptop.

## Infrastructure Is Fragmenting

At the same time, new caching and compute infrastructure is emerging to
serve this need. Some providers focus on artifact storage. Some focus on
remote execution. Some are tied to a build system, a continuous integration
vendor, a cloud runtime, or an agent platform.

The result is useful, but fragmented. Teams still need a way to describe
what work should happen, what values affect it, which outputs matter, and
what an agent can inspect while that work is running, without binding every
automation workflow to one provider.

## Once Is The Narrow Waist

Once is the [narrow waist](https://en.wikipedia.org/wiki/Hourglass_model)
between automation needs and the infrastructure that can make those
workflows faster. Above Once, developers and agents describe targets,
capabilities, and actions. Each action declares its inputs, outputs,
environment, working directory, runtime needs, and required provider
capabilities. Providers can supply local cache storage, shared cache storage,
or remote compute.

Keeping that waist small matters. The action contract should be simple
enough for agents to reason about, stable enough for providers to
implement, and flexible enough for teams to keep using the tools they
already have.

Scripts and typed targets share the same action model. Scripts already encode
real repository knowledge, so Once lets them enter that model immediately.
When that work needs richer dependencies, multiple capabilities,
structured diagnostics, or agent-driven edits, it can move into typed graph
targets while keeping the same cache, build, run, and test workflow.

## What makes reuse trustworthy

Caching is only correct when the action contract is complete. An undeclared
file, a changing network response, or an untracked environment variable can
make a recorded result stale. Once gives you explicit contracts and queryable
plans, but it does not make an arbitrary command hermetic by default.

Start locally, verify the inputs and outputs, and then share the result with
your team. Move work to remote execution when its toolchain and runtime needs
can be supplied by the chosen provider. This keeps performance improvements
attached to a workflow you can inspect and trust.

## Next

Continue with [Getting Started](/guide/getting-started) to install Once and
reuse the result of a cacheable script. If running work away from the current
computer is your main interest, read
[Remote Execution](/guide/infrastructure/remote-execution).
