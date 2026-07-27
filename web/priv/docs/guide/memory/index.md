# Memory

The cache answers whether Once can reuse a result. Memory records evidence about
work that already ran, so a person or tool can inspect its status and provenance
without scraping terminal output.

Once currently records action evidence after:

- `once exec`
- `once run`
- `once build`
- `once test`

Each record identifies its subject, pass or fail status, action digest, cache
state, exit code, captured stream digests, and declared output digests when they
exist. Records belong to the current project and remain queryable across command
invocations.

Evidence does not decide whether a result can be reused. That remains the
action cache's job. Instead, it provides an inspectable account of which action
ran and whether the result was computed or restored.

## Try It

The [Evidence guide](/guide/memory/evidence) continues the scripted workflow
from earlier pages. It runs the same action twice, changes an input, and compares
the resulting records.

## Next

Follow [Evidence](/guide/memory/evidence) for the complete walkthrough, then use
the [Memory reference](/reference/memory/) when you need the record and storage
contract.
