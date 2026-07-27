# Linting

Once turns static analysis into a graph capability with declared inputs,
cacheable reports, normalized findings, and an explicit failure policy. Native
analyzer configuration remains authoritative.

This solves a problem that a plain analyzer invocation leaves to every caller:
the analyzer may use a nonzero process code for valid findings, emit a
tool-specific report, and repeat unchanged work on every machine. Once records
the analyzer version, sources, configuration, command, and report as one
action. A fresh execution and a cache hit produce the same finding result.

## Run a complete example

Start in a repository where Once is installed. Pin Ruff so the analyzer
version is part of the repository toolchain:

```sh
mise use ruff@latest
```

Materialize the bundled Python starter under `quality/python`:

```sh
once edit materialize-example ruff_lint ruff-lint-minimal \
  --destination quality/python
```

The starter creates `quality/python/once.toml` and a small Python source. Its
manifest location and target name produce the target identifier
`quality/python/lint`.

Inspect the target before executing it:

```sh
once query capabilities quality/python/lint
once query schema ruff_lint
```

Run the analyzer:

```sh
once lint quality/python/lint
```

A clean starter prints:

```text
once: lint quality/python/lint complete, 0 errors, 0 warnings, 0 notes
```

Run the same command again. Once restores the analyzer report from the action
cache when its declared inputs and analyzer identity have not changed. Inspect
the latest records to see the first cache miss and following cache hit:

```sh
once query evidence quality/python/lint --limit 2
```

## Add linting to existing sources

A lint target belongs in the package-level `once.toml` beside the sources it
analyzes. For example, `python/once.toml` can declare:

```toml
[[target]]
name = "lint"
kind = "ruff_lint"
srcs = ["src/**/*.py", "tests/**/*.py"]

[target.attrs]
config = ["pyproject.toml"]
args = ["--select", "E,F,I"]
```

Run it with the identifier formed from the manifest directory and target
name:

```sh
once lint python/lint
```

Configuration and data paths are declared action inputs. A source,
configuration, analyzer version, or target-kind implementation change causes
fresh analysis. Unchanged inputs can reuse the report locally or through a
shared cache.

## Choose a built-in analyzer

| Ecosystem | Target kind | Analyzer |
| --- | --- | --- |
| Python | [`ruff_lint`](/reference/prelude/ruff_lint) | [Ruff](https://docs.astral.sh/ruff/linter/) |
| JavaScript and TypeScript | [`eslint_lint`](/reference/prelude/eslint_lint) | [ESLint](https://eslint.org/docs/latest/use/command-line-interface) |
| Go | [`golangci_lint`](/reference/prelude/golangci_lint) | [golangci-lint](https://golangci-lint.run/docs/) |
| Swift | [`swiftlint_lint`](/reference/prelude/swiftlint_lint) | [SwiftLint](https://github.com/realm/SwiftLint) |
| Kotlin | [`detekt_lint`](/reference/prelude/detekt_lint) | [detekt](https://detekt.dev/) |
| Elixir | [`credo_lint`](/reference/prelude/credo_lint) | [Credo](https://hexdocs.pm/credo/) |
| Ruby | [`rubocop_lint`](/reference/prelude/rubocop_lint) | [RuboCop](https://docs.rubocop.org/rubocop/) |

Each target-kind reference lists its attributes, default configuration inputs,
tool resolution behavior, and starter slug. Use the native analyzer
configuration rather than reproducing analyzer settings in Once.

## Control the failure policy

The default policy fails when warnings or errors exist:

```sh
once lint quality/python/lint
```

Change the threshold without changing the analyzer configuration:

```sh
once lint quality/python/lint --fail-on error
once lint quality/python/lint --fail-on note
```

`--fail-on error` reports warnings without failing. `--fail-on note` also
gates informational findings.

Many analyzers use a nonzero process code to mean “findings exist.” Their
target kinds list that code through `success_exit_codes`, so Once captures the
valid report instead of treating the process as broken. Unexpected codes, such
as configuration errors or crashes, fail the action and do not become a
complete lint result.

This distinction matters for automation: a finding is repairable product
data, while a broken analysis is missing evidence.

## Consume normalized findings

Analyzer output is stored as a
[Static Analysis Results Interchange Format report](https://docs.oasis-open.org/sarif/sarif/v2.1.0/sarif-v2.1.0.html).
Once projects it into `once.lint_results.v1`, which contains:

- a completion marker that distinguishes valid analysis from a broken tool;
- counts for errors, warnings, and notes;
- analyzer and rule identity;
- workspace-relative file, line, and column locations;
- optional help links and fingerprints;
- the portable report path.

Request the structured result:

```sh
once --format json lint quality/python/lint
```

Coding agents can request the same result through the `once_lint_target`
[Model Context Protocol](https://modelcontextprotocol.io/) tool when the
server starts with execution enabled:

```sh
once mcp --allow-run
```

See the [`once lint` command reference](/reference/cli/lint) for every command
option.

## Add another analyzer

When no built-in target kind covers a tool, define a project-local Starlark
module. The [Custom lint target kinds](/reference/modules/linting) guide
contains:

- a complete provider and action implementation;
- direct portable-report and native-report adapter patterns;
- analyzer identity, input, output, and exit-code requirements;
- a runnable module starter returned by `once query module-contract`;
- the validation, execution, cache, and evidence checks for a new tool.
