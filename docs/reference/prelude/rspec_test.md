# `rspec_test`

[Ruby Specification (RSpec)](https://rspec.info/) tests run through Once.

## Start Here

Once resolves the Ruby interpreter and the Ruby Specification runner as
separate tools. Each may be a name on the executable search path, an absolute
path, or a workspace-relative path. A common direct installation is:

```sh
gem install rspec
```

Retrieve the runnable starter when you want a complete declaration and sample
specifications:

```sh
once query example rspec_test rspec-test-minimal --format json
```

Copy the declaration below into `once.toml`, adjust `srcs` and the Ruby
environment for the project, then establish discovery and run the file
batches:

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

`rspec_test` invokes the selected Ruby Specification command with its
structured formatter, converts examples into stable target-qualified
identifiers, and writes normalized Once results. Exact execution passes the
native example identifier back to the runner.

The target kind declares Ruby and Ruby Specification as tool requirements.
The runner does not need to be loadable by an unrelated system Ruby
installation.

Automatic batching uses one batch per specification file by default. Set
`batching` to `case` for individual examples, or `target` for one target batch.
A complete discovery run is required before smaller batches are planned.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `ruby` | string | no | `ruby` | Ruby interpreter name, absolute path, or workspace-relative path |
| `runner` | string | no | `rspec` | Ruby Specification runner name, absolute path, or workspace-relative path |
| `config` | list&lt;string&gt; | no | `Gemfile`, `Gemfile.lock` | Dependency and runner configuration inputs |
| `data` | list&lt;string&gt; | no | `[]` | Runtime data and support inputs |
| `args` | list&lt;string&gt; | no | `[]` | Additional runner arguments |
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
kind = "rspec_test"
srcs = ["spec/**/*_spec.rb"]

[target.attrs]
env_inherit = ["GEM_HOME", "GEM_PATH"]
```

List helper files and other runtime inputs in `data` when they are outside the
source patterns.
