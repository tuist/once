# `once query test-plan`

Create an immutable test plan without assigning work to runners

## Synopsis

```text
once query test-plan [OPTIONS]
```

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `--changed-path` | `<PATH>` |  | Changed workspace-relative path. Repeat for multiple paths |
| `--target` | `<TARGET>` |  | Create an explicit plan for this target instead of selecting by changed path |
| `--test-unit` | `<TEST_UNIT>` |  | Select one current, filterable unit from `once query test-manifest`. Planning fails when the target does not support exact filtering or the unit is absent from the persisted whole-target manifest |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |
