# Memory Reference

Memory is project-local runtime state. It belongs to the workspace that
produced it and is safe to remove when you want Once to forget local
history.

## Storage

Once stores project memory under `.once/`. Evidence records are written
to `.once/once.sqlite`.

The SQLite database stores structured provenance, not large process
output. For evidence records, stdout, stderr, and declared outputs are
stored as content digests. The byte payloads stay in the
content-addressed store.

## Location

The memory database is intentionally workspace-local. `XDG_CACHE_HOME`,
`XDG_STATE_HOME`, and related XDG variables are used for user-global
state such as the shared content-addressed cache, logs, credentials, and
future per-user runtime files. They do not move project memory out of
the checkout.

Use `-C, --directory` on CLI commands, or `once mcp --workspace`, to
choose which workspace owns the memory being read or written.

Once does not currently expose an `ONCE_` environment override for the
memory database location. If that becomes necessary, it should be an
explicit workspace-state override rather than an implicit XDG fallback,
because evidence is meaningful only relative to the graph and files in a
specific workspace.
