# Event-Driven Automation

Declare when work should run in `struo.toml`. Your CI service delivers the event
and schedules a worker; Struo runs the selected capabilities.

A build graph already describes what to build, how to test it, and what must
run first. A trigger selects from that graph instead of describing the same
work again as CI jobs. Changing a target's dependencies should not require
editing a second pipeline.

```text
Commit → CI service
         ↓
         Struo trigger
         ↓
         Capabilities → Result
```

An integrated service can read these declarations directly. A service with
dynamic pipelines can generate worker jobs from them. In either case, the
build graph stays in Struo, not in a hand-maintained pipeline YAML file.

::: info Tuist support is coming
Tuist orchestration support for Struo triggers is coming. It is not available
yet. Tuist's existing [shared-cache integration](/guide/infrastructure/tuist)
is separate from trigger orchestration.
:::

## Declare A Trigger

Add a trigger to your root `struo.toml`:

```toml
[[trigger]]
id = "verify-main"
on = { event = "vcs.ref_changed", refs = ["refs/heads/main"] }

[[trigger.invoke]]
target = "checks/core"
capability = "test"
configuration = { os = "linux", arch = "x86_64", tokens = [] }
```

This asks the service to run the `test` capability of `checks/core` when
`main` changes. Struo handles the target's dependencies, actions, and caching.
The declaration does not repeat the commands used to build or test it.

- `id` is the trigger's stable name.
- `refs` contains exact, fully qualified refs, not branch patterns.
- `target` must already exist and provide the selected capability.
- `configuration` explicitly sets the target OS, architecture, and extra
  configuration tokens. The service must provide a compatible worker.

`checks/core` is an example. Use `struo query capabilities <target>` to inspect
your own target. Triggers support `build`, `test`, and `run` on analysis-backed
target kinds. Add more `[[trigger.invoke]]` entries to select more work, not
another set of dependency edges.

See [target configuration](/guide/graph/configuration) and
[test execution](/guide/graph/testing) for the selected work.

## Services And Integration Status

There is no released native trigger integration yet. The current CLI provides
an experimental worker interface that services can integrate with.

- **Tuist: coming.** Planned service integration that reads Struo triggers.
- **[Buildkite](/guide/infrastructure/buildkite): example adapter.** Generate
  JSON steps and upload them to the running build. One worker job per matching
  trigger, not per build action. No repository pipeline YAML is needed.
- **[Woodpecker CI](https://woodpecker-ci.org/docs/usage/extensions/configuration-extension): possible.**
  A configuration extension returns generated worker pipelines. Exclusive
  mode needs no repository pipeline file.
- **[Jenkins](https://www.jenkins.io/doc/book/pipeline/getting-started/): possible.**
  A centrally configured Scripted Pipeline can create parallel workers from
  trigger data. No application `Jenkinsfile` is needed when the entrypoint
  lives in Jenkins.
- **[GitLab CI](https://docs.gitlab.com/ci/pipelines/downstream_pipelines/#dynamic-child-pipelines): possible.**
  Generate a child pipeline. A parent pipeline and generated YAML remain
  necessary.
- **[CircleCI](https://circleci.com/docs/guides/orchestrate/dynamic-config/): possible.**
  Generate continuation configuration. A setup configuration remains necessary.
- **[GitHub Actions](https://docs.github.com/en/actions/reference/evaluate-expressions-in-workflows-and-actions#fromjson): possible.**
  Expand a job matrix from bootstrap JSON with `fromJSON`. A workflow YAML and
  its event subscriptions remain necessary.
- **[Azure Pipelines](https://learn.microsoft.com/en-us/azure/devops/pipelines/process/phases?view=azure-devops#multi-job-configuration): possible.**
  Expand a runtime JSON matrix. A pipeline YAML and authorization for eligible
  agent pools remain necessary.

**Possible means a custom adapter could use the platform's existing
capabilities.** Struo does not ship adapters for those services yet.

Buildkite, Woodpecker, and Jenkins can keep the adapter entrypoint outside the
application repository. Woodpecker's configuration extension is the closest
fit for a service that directly asks an adapter what to run. Other services
may still require a small bootstrap file. Generating configuration for the
service's API is fine; maintaining another copy of the build graph is not.

Adapters must keep service credentials and privileged pipeline code outside
candidate control. For example, Woodpecker extensions receive signed requests
and may receive repository credentials; Jenkins trusted libraries can have
controller privileges. Neither should evaluate project code to select work.

For matrix-based adapters, keep a stable required completion check that fails
when discovery or a worker fails. Handle no matching triggers explicitly;
an empty matrix does not reliably mean zero jobs across services. Native
event subscriptions must still start the bootstrap whenever Struo work could
be required.

## How A Service Integrates

1. **Register approved triggers.** Read a reviewed, committed definition
   revision separately from the commit being tested. A candidate must not be
   able to skip its own tests by deleting or narrowing its trigger.
2. **Select and schedule work.** Translate the service's authenticated event
   into a source ref and full commit ID, match the registered triggers, and
   schedule compatible workers. Reading trigger data does not evaluate project
   modules.
3. **Run Struo and publish the result.** On a clean checkout of that exact
   commit, invoke the selected trigger. Struo resolves and executes its graph;
   the service handles job lifecycle, isolation, credentials, and required
   checks. Missing capabilities and failed invocations fail the run.

The [Buildkite integration example](/guide/infrastructure/buildkite) shows how
to connect these steps to an existing orchestrator. A native controller's
proposed lifecycle is covered in the
[engineering loops RFC](https://github.com/tuist/struo/blob/main/rfcs/0002-engineering-loops.md#event-registrations).

## Inspect And Edit Triggers

These commands read, validate, or edit trigger declarations without running
project modules:

```sh
struo query triggers
struo query validate-trigger --file draft.json
struo edit triggers --file changes.json
```

`query triggers` includes JSON schemas for trigger drafts and event files.
Edits use `{ "operations": [...] }` with `create`, replacement `update`, or
`delete` operations. Invalid batches leave the manifest unchanged; successful
edits still need separate activation.

The same operations are available through [MCP tools](/reference/mcp/tools).
Editing, activation, and execution require `struo mcp --allow-run`.

::: warning Experimental worker interface
Struo does not yet receive webhooks, provision workers, or publish CI checks.
The current run mode is called simulation, but it executes real project code
with the worker's permissions. Event JSON and local activation are not proof
of source trust or a security boundary. CI must isolate execution before
checkout and tool setup, keep credentials outside it, and enforce resource
ceilings.

Runs use a dedicated local cache, not the configured shared cache or live
reporter. They do not provide immutable replay or crash recovery. Do not use
them for deployments or other external effects.
:::
