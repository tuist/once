# Memory Reference

Memory is project-local runtime state. It belongs to the workspace that
produced it and is safe to remove when you want Once to forget local
history.

## Storage

Once stores project memory under `.once/`. Evidence records are stored in
`.once/once.sqlite`.

The database records what ran, its status, its cache decision, and identifiers
for command output and declared outputs. Cached content stays in the shared
content-addressed store instead of being duplicated in project memory. See
[Evidence](/guide/memory/evidence) for the records you can query.

## Location

The memory database is workspace-local. The `XDG_CACHE_HOME` and
`XDG_STATE_HOME` variables from the
[freedesktop.org base directory specification](https://specifications.freedesktop.org/basedir-spec/latest/)
can move user-level caches and state, but they do not move project memory out
of the workspace.

Use `-C, --directory` on command-line operations, or
[`once mcp --workspace`](/reference/cli/mcp), to choose which workspace owns
the memory being read or written.

The memory location cannot be configured independently from the workspace.
Evidence remains associated with the graph and files that produced it.
