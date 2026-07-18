# `pytest_test`

Python tests run with pytest.

## Start Here

The selected `python` interpreter must be able to import pytest. When the
attribute is omitted, Once first uses `.venv/bin/python` or the Windows virtual
environment equivalent when present, then tries `python3` and `python` on the
executable search path. You can also set an absolute or workspace-relative
path explicitly. A common direct installation is:

```sh
python3 -m pip install pytest
```

Retrieve the runnable starter when you want a complete declaration and sample
tests:

```sh
once query example pytest_test pytest-test-minimal --format json
```

Copy the declaration below into `once.toml`, adjust `srcs` for the project,
then establish discovery and run the file batches:

```sh
once test tests --format json
once query test-manifest tests --format json
once test tests --jobs 4 --format json
```

The first command runs the complete target. Later runs can use the resulting
manifest for automatic batching and exact selection. See
[Testing and Scheduling](/guide/graph/testing) for affected plans, case-level
batching, and exact unit commands.

## Description

`pytest_test` invokes pytest through the selected Python interpreter, records
stable node identifiers, and writes normalized Once results. A complete run
discovers cases for exact interactive execution and automatic scheduling.

Automatic batching uses one batch per test file by default. Set `batching` to
`case` for one batch per discovered case, or `target` to keep one batch for the
target. A missing or stale manifest runs the whole target once before Once
uses smaller batches.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `python` | string | no | automatic | Python interpreter name, absolute path, or workspace-relative path that can import pytest |
| `config` | list&lt;string&gt; | no | `pyproject.toml`, `pytest.ini`, `conftest.py` | Configuration and collection inputs |
| `data` | list&lt;string&gt; | no | `[]` | Runtime data inputs read by tests |
| `args` | list&lt;string&gt; | no | `[]` | Additional pytest arguments |
| `env` | map&lt;string, string&gt; | no | `{}` | Explicit test environment variables |
| `env_inherit` | list&lt;string&gt; | no | `[]` | Host environment variables inherited by name |
| `batching` | string | no | `file` | Automatic granularity: `file`, `case`, or `target` |
| `labels` | list&lt;string&gt; | no | `[]` | Labels exposed through test discovery |
| `timeout_ms` | int | no | empty | Optional timeout in milliseconds |

## Dependency Edges

`deps` names targets whose changes should select this test target.

## Providers and Capabilities

The target emits `once_test_info` and exposes `test` with the `default`,
`test_results`, and `logs` output groups.

## Example

```toml
[[target]]
name = "tests"
kind = "pytest_test"
srcs = ["tests/**/*.py"]

[target.attrs]
python = ".venv/bin/python"
labels = ["unit"]
```

Declare fixtures, configuration, and runtime files through `srcs`, `config`,
or `data` so they participate in caching and manifest freshness.
