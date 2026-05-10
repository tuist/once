# Agent-Native Features

Fabrik should not stand out as "Bazel, but easier." The stronger position is:

> Fabrik is the build system designed for coding agents and humans to understand, repair, and operate.

Agents are effective when a system exposes structured data, schemas, precise causality, local reproductions, and machine-applicable patches. Fabrik should make those the default interface to builds, tests, runtime tasks, caching, and remote execution.

## Standout Features

### Build Doctor

Every failed build, test, or runtime task should produce a structured diagnosis object, not only logs.

The diagnosis should include:

- exact failing target and action
- command, cwd, environment, inputs, and outputs
- sandbox contents and undeclared access attempts
- dependency path that caused the action to run
- cache status and cache miss reason
- resource usage and scheduler state
- root cause category
- minimal reproduction command
- suggested manifest patch when Fabrik can infer one

This lets agents debug failures without scraping stderr or reverse-engineering the action graph from logs.

### Why API

Fabrik should expose direct causality APIs:

```sh
fabrik why app/ios
fabrik why-rebuilt app/ios
fabrik why-cache-miss app/ios
fabrik why-failed app/ios
fabrik why-slow app/ios
fabrik impact Sources/App/View.swift
```

These commands should answer questions in terms of concrete changed inputs, environment changes, dependency paths, output declarations, cache keys, scheduler decisions, and plugin boundaries.

### Debug Capsules

Every action should be able to emit a portable debug capsule:

```text
capsule/
  action.json
  inputs/
  outputs/
  stdout.txt
  stderr.txt
  env.json
  timing.json
  cache.json
  resources.json
  reproduce.sh
```

A capsule gives humans, agents, CI systems, and remote executors the exact context needed to reproduce and inspect a failure.

### First-Party Importers With Repair

Fabrik should not ask users or agents to hand-port entire ecosystems first. Official importers should exist for native manifests:

```sh
fabrik import cargo
fabrik import xcode
fabrik import gradle
fabrik import bazel
```

Importers should explain their output, check drift, and propose repairs:

```sh
fabrik import cargo --explain
fabrik import cargo --check
fabrik import cargo --repair
```

The target outcome is a validated, idiomatic `fabrik.toml` graph with explicit gaps when full fidelity is not possible.

### Schema-Driven Plugins

Every plugin should expose:

- TOML schema
- generated docs
- examples
- validation rules
- action graph preview
- diagnostics taxonomy
- importer hooks
- cacheability contract
- remote execution contract

Agents should be able to discover how to write a target from Fabrik itself:

```sh
fabrik schema rust.binary
fabrik explain-schema apple.ios_app
fabrik examples rust.test
fabrik validate
```

### Runtime Observability

Runtime tasks should be first-class. `fabrik run`, `fabrik test`, and checked-in task targets should expose structured state for live execution, retries, timeouts, side effects, simulator and process metadata, captured artifacts, and test failures.

For an iOS app, a failed launch should report whether the simulator was unavailable, bundle installation failed, the app crashed, signing failed, a runtime was missing, or boot timed out.

### Cache Explainability

Remote and local caching should be explainable:

```sh
fabrik cache explain app/ios
fabrik cache diff <action-a> <action-b>
fabrik cache tree app/ios
```

The primary question is simple: why was this not a cache hit?

### Agent Repair Mode

Fabrik should produce candidate patches for common declaration and execution problems:

```sh
fabrik repair app/ios
fabrik repair --dry-run
```

Repair mode should cover missing sources, wrong dependencies, undeclared outputs, non-hermetic environment use, toolchain mismatches, cache-disabled actions, importer gaps, and malformed TOML.

### Plan Before Execution

Agents need to inspect the consequences of a change before running expensive work:

```sh
fabrik plan app/ios
fabrik plan --json app/ios
fabrik plan --changed-since main
```

The plan should show actions that would run, expected cache hits, local versus remote placement, resource estimates, critical path, affected tests, missing declarations, and non-cacheable actions.

### Resource and Cost Governance

Fabrik should make resource bounds and scheduling visible:

```sh
fabrik run --budget.cpu=8 --budget.memory=16g --budget.remote-cost=5
```

Users and agents should be able to ask why an action waited, why it ran locally or remotely, why prefetch happened, what resource blocked progress, and which actions were on the critical path.

## Highest-Value Bets

The five features most likely to make Fabrik stand out are:

1. Build Doctor
2. Why API
3. Debug Capsules
4. First-party importers with repair
5. Schema-driven plugin system

Fast builds, remote execution, and caching are required, but they are not enough to differentiate Fabrik from Bazel. The strongest product bet is making build systems debuggable and editable by agents.
