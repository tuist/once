# Build with Once

A fast build is often a build you do not have to run. Once gives work a stable
identity so a result produced by you, a teammate, or a coding agent can be
reused instead of repeated.

Start with [Getting Started](/guide/getting-started). In a small, self-contained
example, you will run a script, reuse its result, restore a deleted output, and
see an input change trigger fresh work. No account or remote service is needed.

Then choose the path that fits your repository. Once can read supported native
workspace metadata, cache the automation that already works, and add a typed
graph where more structure helps. You do not need to adopt all three at once.

## Start with an existing project

Once can recognize supported native workspaces and make their build, test, and
run capabilities available without requiring changes to their project files.
Native project descriptions and lockfiles remain authoritative. Read the
[Typed Graph guide](/guide/graph/#start-from-a-native-project) to discover and
preview supported projects without adding Once configuration.

## Cache existing automation

Use [Scripted automation](/guide/scripted/) to add explicit inputs, outputs,
environment, and cache policy to commands or scripts that already work. The
existing script remains the source of truth, while Once caches operations such
as packaging, generation, and checks.

## Add structure when it helps

Use the [Typed Graph guide](/guide/graph/) for named targets, typed
dependencies, build and run capabilities, and ecosystem integrations. You can
keep native discovery and cacheable scripts beside typed targets while the
graph grows. Add a package manifest when you want to keep a choice in the
repository. Introduce a project [Starlark](https://starlark-lang.org/) module
only when the built-in target kinds cannot express reusable behavior your
project needs.

- [Testing and scheduling](/guide/graph/testing) covers normalized test
  results, selection, batching, retries, and custom test runners.
- [Linting](/guide/graph/linting) covers cacheable static analysis, shared
  finding results, failure policy, built-in analyzers, and custom analyzer
  target kinds.
- [Ecosystems](/guide/graph/ecosystems) helps choose between typed targets and
  scripted automation.

## Run work where it makes sense

Once runs actions locally first. As a project grows, control how much work runs
at once, then share cached results or move suitable work to remote execution.

- **[Control local execution](/guide/local-execution/memory-limits):** use
  Memory Limits to keep concurrent local actions within a scheduling budget on
  constrained or shared machines.
- **[Understand unchanged builds](/guide/local-execution/unchanged-builds):**
  see how Once establishes that there is nothing to do, and what to set when
  there is no series of builds to amortise a watcher over.
- **[Share or move execution](/guide/infrastructure/):** add shared cache
  entries, isolated sandboxes, or hosted execution after the local workflow is
  working well.

## Bring Once into your workflow

The command line works on its own. Use the following integrations when another
application or a coding agent needs to work with the same cache, graph, and
execution model.

- **[Integrate Once into an application](/guide/sdk/):** use a language library
  when application code needs direct access to Once cache primitives.
- **[Connect a coding agent](/guide/harness):** give a coding agent query,
  editing, execution, validation, and evidence tools through the
  [Model Context Protocol](https://modelcontextprotocol.io/).
