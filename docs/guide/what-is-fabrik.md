# What is Fabrik

Fabrik is an execution layer between developers and coding agents on
one side, and cache and compute infrastructure on the other. Teams
define software operations once, and Fabrik turns them into
content-addressed actions with explicit inputs, outputs, environment
dependencies, and runtime semantics.

::: warning Beta
Fabrik is beta software. The CLI, local cache, and current target model are usable, but target schemas and plugin behavior can still change.
:::

## Why this layer exists

Most repositories already have two different worlds that barely talk to
each other. At the top, developers and agents work through scripts,
manifests, build tools, and one-off commands. At the bottom, caches,
local processes, sandboxes, and remote workers need deterministic units
of work they can hash, schedule, replay, and restore.

Fabrik sits in that gap. It standardizes the execution contract so the
people and agents above it do not need to understand the details of the
infrastructure below it, and the infrastructure below it does not need
to scrape ad hoc scripts or tool-specific state.

That separation matters because it gives the repository one operational
surface. A human can review it. An agent can patch it safely. A cache
or compute backend can execute it without guessing what the work meant.

## What Fabrik standardizes

Fabrik projects are made of `fabrik.toml` files placed next to the code
they describe. Those files declare targets, dependencies, and script
actions as structured data instead of shell glue.

Fabrik lowers that definition into actions with declared inputs,
declared outputs, environment dependencies, working directories,
resource hints, and runtime metadata. That is the contract the cache and
executor consume, and it is also the contract humans and agents can
inspect when they need to understand why something ran or why a cache
hit did not happen.

## Scripts belong in the layer

Real repositories are full of important scripts: asset bundlers,
generators, test setup, packaging flows, fixture builders, and internal
developer tooling. Those scripts usually sit outside the build graph,
which makes them hard to cache, hard to inspect, and hard to move onto
other execution backends cleanly.

Fabrik treats them as part of the same operational layer. A script can
stay in a checked-in file with `FABRIK` headers, or it can be declared
directly in `fabrik.toml` when it is short enough to stay inline. In
both cases, it becomes a normal Fabrik action with explicit inputs,
declared outputs, cache behavior, and runtime semantics.

## Why that helps both people and infrastructure

When a repository has that shared contract, humans and agents get a
clearer system to work on. They can ask why an action ran, what inputs
were tracked, what outputs were restored, and what changed since the
last green run without digging through improvised wrappers or opaque
build logs.

The infrastructure underneath also gets a better deal. Cache systems and
executors receive deterministic units of work instead of tool-specific
side effects. That is what lets the same repository definition stay
useful whether it runs on a laptop today or a different compute backend
later.

## What makes Fabrik different

Fabrik is not just another way to write build files. It is a boundary
between repository automation and execution infrastructure.
That boundary is structured enough for agents to edit safely, concrete
enough for caches to trust, and flexible enough to cover both native
rules and existing script-driven workflows. The goal is not to replace
every tool in a repository. The goal is to give those tools one
execution model that can be optimized coherently.
