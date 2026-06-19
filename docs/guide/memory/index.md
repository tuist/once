# Memory

Once starts from a project graph: targets, dependencies, capabilities,
providers, and typed schemas. The graph tells Once what exists and what
can be done.

Memory records what happened around that graph. It is project-local
state that helps humans, scripts, and agents answer practical questions
without scraping terminal output or replaying work blindly:

- Which commands or capabilities ran?
- What passed or failed?
- What evidence exists for a target or capability?
- Is that evidence still fresh for the current inputs?
- What should run next?

Memory lives under `.once/` because it is runtime state, not
source-controlled intent. It can be rebuilt by running work again, but
while it exists it gives Once durable working memory for the project.

## Memory Guides

- [Evidence](/guide/memory/evidence): durable provenance for action
  outcomes, including how records are created, queried, and used for
  planning.

More memory guides will land as Once grows from action outcomes into
run history, derived views, and long-running feedback loops.
