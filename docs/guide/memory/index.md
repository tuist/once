# Memory

Once starts from a project graph: targets, dependencies, capabilities,
providers, and typed schemas. The graph tells Once what matters.

Memory records what happened around that graph. It is project-local
state that lets humans, scripts, and agents answer practical questions
without scraping terminal output or replaying work blindly:

- Which commands or capabilities ran?
- What passed or failed?
- What evidence exists for a target or capability?
- Is that evidence still fresh for the current inputs?
- What should run next?

Memory lives under `.once/` because it is runtime state, not
source-controlled intent. It can be rebuilt by running work again, but
while it exists it gives Once durable working memory for the project.

## Shape

The useful public model is:

| Concept | Meaning |
| --- | --- |
| Graph | What the repository declares. |
| Runs | What Once executed. |
| Evidence | Durable claims produced by runs, tools, policies, agents, or humans. |
| Views | Answers computed from graph, runs, and evidence. |
| Loops | Repeatable workflows that use those answers over time. |

The first implemented piece is evidence for action outcomes. See
[Evidence](/guide/memory/evidence) for how it works today.
