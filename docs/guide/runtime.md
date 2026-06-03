# Runtime

Once exposes runtime sessions for tools and agents that need structured
access to command output and metadata. Instead of scraping a terminal log,
an agent can query logs, events, and runtime descriptors over the local
JSON-RPC control socket.

Runtime sessions are useful when a script starts something long-lived or
interactive, such as a development server, worker, simulator, or local
service. The script still declares its execution contract with `# once`
headers, while runtime metadata describes the controls and interfaces that
other tools can use after the action starts.

Use `once runtime` to inspect and control active sessions.
