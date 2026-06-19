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

## Why Evidence Exists

Build caches are good at reusing outputs, but engineering work often
needs a different answer:

```text
Can I trust that this target was tested?
Was that result produced for the inputs I have now?
Did it pass locally or come from cache?
What should an agent run next?
```

Evidence is the first piece of memory that answers those questions. It
records action outcomes as durable provenance: the subject, status,
action digest, input digest when available, cache state, exit code, and
captured output digests.

That gives Once a fact base for planning. A coding agent can avoid
running the whole suite when fresh evidence already exists, and it can
identify the smallest missing or stale check after a change.

See [Evidence](/guide/memory/evidence) for the current command surface.
