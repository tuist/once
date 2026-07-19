# `once query validate-actions`

Run scripted graph actions in an uncached isolated validation workspace and return structured filesystem contract repairs.

## Synopsis

```text
once query validate-actions <TARGET> [OPTIONS]
```

The probe runs the selected capability without consulting the action cache and does not materialize outputs. It checks changes in the isolated action workspace and the project workspace, reports input mutation, undeclared writes, missing outputs, and path escapes, and includes a repair for each diagnostic. Reads that leave no observable filesystem evidence cannot be reported by this validation mode.

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `--capability` | `<CAPABILITY>` | `build` | Capability to validate. |
| `--action` | `<INDEX>` |  | Validate one zero-based action index. |
| `-C, --directory` | `<DIR>` |  | Project root. |
| `--format` | `<FORMAT>` | `human` | Output format. Use `json` for agent consumers. |

## Positional arguments

| Name | Description |
| --- | --- |
| `TARGET` | Canonical target id whose scripted capability should be probed. |
