# Guides

Choose the smallest Once surface that solves the problem in front of you.
Scripts are the fastest migration path. Typed graph targets add queryable
dependencies and capabilities when a workflow needs more structure.

## Start with existing automation

Use [Scripted automation](/guide/scripted/) to add explicit inputs, outputs,
environment, and cache policy to commands or scripts that already work.

## Model builds and repository capabilities

Use the [Typed Graph guide](/guide/graph/) for named targets, typed
dependencies, build and run capabilities, and ecosystem integrations.

- [Testing and scheduling](/guide/graph/testing) covers normalized test
  results, selection, batching, retries, and custom test runners.
- [Linting](/guide/graph/linting) covers cacheable static analysis, shared
  finding results, failure policy, built-in analyzers, and custom analyzer
  target kinds.
- [Ecosystems](/guide/graph/ecosystems) helps choose between typed targets and
  scripted automation.

## Control local execution

Use [Memory Limits](/guide/local-execution/memory-limits) to keep concurrent
local actions within a scheduling budget on constrained or shared machines.

## Share or move execution

Use [Infrastructure](/guide/infrastructure/) after local execution works and
you want shared cache entries, isolated sandboxes, or hosted execution.

## Integrate Once into an application

Use the [language library overview](/guide/sdk/) when application code needs
direct access to Once cache primitives.

## Connect a coding agent

Use [Coding harnesses](/guide/harness) to give a coding agent query, editing,
execution, validation, and evidence tools through the
[Model Context Protocol](https://modelcontextprotocol.io/).
