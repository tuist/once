# Reference

Use the reference when you need an exact command, manifest field, target
contract, or protocol behavior. Start with a guide when you are learning a
workflow for the first time.

- The [`once.toml` Manifest](/reference/manifest) covers workspace discovery,
  root-only configuration, target declarations, and path resolution.
- The [Command-line Reference](/reference/cli/) covers every subcommand the
  `once` binary exposes, with its synopsis, options, and arguments.
- The [Modules Reference](/reference/modules/) documents how projects define
  target kinds and actions.
- The [Target-kind Reference](/reference/prelude/) enumerates every
  built-in target kind, including its attributes, dependency edges,
  capabilities, and outputs.
- The [Memory Reference](/reference/memory/) documents project-local
  runtime state, the evidence database, and location rules.
- The [Model Context Protocol Reference](/reference/mcp/) documents the
  [Model Context Protocol](https://modelcontextprotocol.io/) surface that
  `once mcp` serves: transport, handshake, error
  model, an example session, and the complete tool catalog.
- The [Live Run Event Protocol Reference](/reference/events/) documents
  the gRPC surface Once uses to stream typed events to a compatible
  ingest server: what the client sends, delivery guarantees, the event
  catalog, redaction rules, and how to enable ingest.
