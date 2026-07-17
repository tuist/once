# `pytest_test`

Python tests run with pytest.

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
| `python` | string | no | `python3` | Python interpreter that can import pytest |
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
labels = ["unit"]
```

pytest must be importable by the selected interpreter. Declare fixtures,
configuration, and runtime files through `srcs`, `config`, or `data` so they
participate in caching and manifest freshness.
