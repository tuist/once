# What is Fabrik

Fabrik is a polyglot, agent-native build system. It uses content-addressed actions, structured declarations, and explicit runtime semantics so humans and coding agents can build, run, test, and debug the same graph.

::: warning Beta
Fabrik is beta software. The CLI, local cache, and current target model are usable, but target schemas and plugin behavior can still change.
:::

## Why Fabrik

Build systems are becoming shared infrastructure for humans, coding agents, and remote compute. That creates a chance to rethink three old boundaries at once: who governs the build system, who can understand it, and where the graph is allowed to run.

Fabrik is designed as a community-governed build system where the core and first-party rules evolve together. Rules for important ecosystems should be authored, tested, and maintained with the same care as the scheduler, cache, CLI, and diagnostics. The community should shape the system, not inherit the maintenance burden after the core decisions are made.

Fabrik is also designed to make the build graph accessible to both people and agents. Declarations should be structured enough for tools to edit safely, diagnostics should explain causality instead of dumping logs, and every target should be inspectable through stable APIs:

- Why did this action run?
- Why was this not a cache hit?
- What changed since the last green build?
- What is the minimal reproduction of this failure?

Fabrik treats those questions as first-class APIs, not log-scraping exercises.

The graph definition should also be separate from where the graph executes. A project should describe what needs to happen once, then let local machines, remote workers, and future compute substrates execute the same graph without rewriting the build. That separation is how build systems break through the ceiling of a single workstation.

## The graph

Fabrik projects are made of `fabrik.toml` files placed next to the code they describe. Those files declare targets and relationships, so the graph can be reviewed by humans, edited by agents, and validated by tools without scraping commands or logs.

Fabrik lowers that graph into content-addressed actions with declared inputs, declared outputs, resource needs, and runtime semantics. The same project definition can run on a laptop, a remote worker, or a future execution backend because the graph describes what needs to happen, not where it must happen.

See [Project layout](./project-layout.md) for how build files, directory scopes, targets, and target IDs fit together. See [Cache and execution](./cache-and-execution.md) for the execution model.

## What makes Fabrik different

- **Structured declarations:** build files are data, so tools can inspect, validate, and rewrite them safely.
- **Content-addressed execution:** actions are keyed from declared inputs and restore declared outputs from the cache.
- **Rules with shared ownership:** important ecosystems can have first-party rules maintained with the core while governance stays open.
- **Agent-readable diagnostics:** the graph exposes causality, cache decisions, and repair context as APIs.
- **Portable compute:** the graph definition is separate from the place where the work executes.

## Where Fabrik is going

Fabrik is moving toward a build graph that acts as shared infrastructure for humans, agents, and elastic compute. The [design](../reference/design.md), [roadmap](../reference/roadmap.md), [rules](../reference/rules.md), and [runtime target](../reference/runtime.md) references describe the longer-term direction.
