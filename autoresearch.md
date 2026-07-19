# Autoresearch: validate scripted action filesystem contracts

## Objective

Improve Once's ability to report whether a scripted graph action's declared workspace inputs and outputs match its filesystem behavior. Retain only changes that improve detection without introducing false positives in the control corpus.

## Metrics

- Primary: detection harmonic mean of precision and recall (`detection_f1_pct`, percentage, higher is better)
- Secondary: `precision_pct`, `recall_pct`, `actionable_diagnostic_rate_pct`, `false_positive_rate_pct`, `runtime_overhead_pct`, and `benchmark_ms`

Precision is true positives divided by all reported violations. Recall is true positives divided by all seeded violations. Actionable-diagnostic rate is seeded violations that produce a path-specific structured repair divided by all seeded violations. False-positive rate is compliant cases reported as violations divided by all compliant cases. Runtime overhead is the median private-input-sandbox runtime relative to unsandboxed execution across nine paired runs.

## How to Run

`./autoresearch.sh`

## Workload

The fast corpus contains eight adversarial actions and four compliant controls. The adversarial set covers a relative undeclared read, an extra write, declared-input mutation and deletion, absolute workspace read and write, a declared symbolic-link escape, and an output symbolic-link escape. The controls include the vendored open-source Rust `itoa` source, a JavaScript test source, a declared source directory, and a nested declared output.

## Files in Scope

- `crates/once-core/src/execute.rs`: private execution root staging and output copy-back
- `crates/once-core/src/contract.rs`: generic contract observations
- `crates/once-core/src/runner.rs`: uncached validation entry point
- `crates/once-cli/src/commands/graph/`: declared-action validation orchestration
- `crates/once-cli/src/commands/query.rs`: matching command-line query
- `crates/once-cli/src/commands/mcp.rs`: agent tool transport
- `crates/once-frontend/src/analysis/`: public Starlark declaration contract
- `docs/reference/`: public command, tool, and Starlark module references

## Off Limits

- Ecosystem-specific branches in Rust execution code
- Treating dependency files as complete access evidence
- Claiming successful undeclared reads are observed by a symbolic-link tree
- Remote execution behavior

## Constraints

- Keep correctness checks separate in `autoresearch.checks.sh`.
- Use `mise exec --` for every Rust command.
- Return stable diagnostics with target, attribute, and repairs.
- Give every Model Context Protocol tool a matching `once query` or `once edit` command.
- Do not use em dashes in user-facing text.

## What's Been Tried

- The initial baseline scores only action failure as a detected contract violation. It intentionally measures the existing symbolic-link sandbox before adding post-run observation.
