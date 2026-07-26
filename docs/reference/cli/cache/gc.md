# `once cache gc`

Reclaim local cache space down to a size budget

## Synopsis

```text
once cache gc [OPTIONS]
```

## Description

Removes the oldest blobs and action results until the local store fits within `--max-size`. Safe to run at any time: an action whose outputs get evicted simply re-executes on its next build. Only the local tier is collected; a remote cache is never touched.

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `--max-size` | `<SIZE>` |  | Maximum local cache size to keep. Plain bytes or a human suffix: `500MB`, `2GiB`, `750k`. Decimal suffixes (KB/MB/GB/ TB) are powers of 1000; binary suffixes (KiB/MiB/GiB/TiB) are powers of 1024 |
| `--dry-run` | (flag) | `false` | Report what would be reclaimed without deleting anything |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |
