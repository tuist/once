# `rust_test`

Rust test target.

## Description

Compiles a Rust test crate with `rustc --test`, then runs the produced test
binary through Once's generic test capability. The target can model unit tests
from a library root, or integration tests by setting `crate_root` to a test
crate entry file and depending on the library under test.

The runner uses the Rust test harness list output to populate
`once.test_results.v1`, stores the native test output, and returns the test
binary exit status to Once.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `crate_name` | string | no | target name | Rust crate name passed to `rustc`; `-` and `.` are rewritten as `_` when omitted |
| `crate_root` | string | no | `src/lib.rs` | Package-relative test crate root |
| `edition` | string | no | `2021` | Rust edition passed to `rustc` |
| `features` | list&lt;string&gt; | no | `[]` | Cargo feature names lowered to `--cfg=feature=...` flags |
| `crate_features` | list&lt;string&gt; | no | `[]` | Bazel-compatible alias for `features` |
| `target` | string | no | host target | Rust target triple passed to `rustc --target` |
| `env` | map&lt;string, string&gt; | no | `{}` | Environment variables for `rustc` |
| `rustc_env` | map&lt;string, string&gt; | no | `{}` | Rust compiler environment variables |
| `rustc_env_files` | list&lt;string&gt; | no | `[]` | Files with `NAME=value` entries merged into the rustc environment before `env` and `rustc_env` |
| `rustc_flags` | list&lt;string&gt; | no | `[]` | Additional `rustc` flags appended after Once-managed flags |
| `cap_lints` | string | no | empty | Optional rustc lint cap passed as `--cap-lints` |
| `linker` | string | no | inferred | Optional linker path passed as `-C linker=...` |
| `linker_flags` | list&lt;string&gt; | no | `[]` | Additional linker flags lowered to `-C link-arg=...` |
| `native_linkopts` | list&lt;string&gt; | no | `[]` | Linker flags propagated to downstream native consumers when this target is used as native input |
| `exported_linker_flags` | list&lt;string&gt; | no | `[]` | Buck-compatible alias for native linker flags propagated to downstream native consumers |
| `exported_post_linker_flags` | list&lt;string&gt; | no | `[]` | Buck-compatible propagated linker flags appended after normal exported linker flags |
| `linker_script` | string | no | empty | Package-relative linker script passed to the linker and included in the compile action inputs |
| `data` | list&lt;string&gt; | no | `[]` | Runtime data file globs available to the test runner and propagated from Rust dependencies |
| `compile_data` | list&lt;string&gt; | no | `[]` | Bazel-compatible compile-time data file globs included in the rustc action inputs |
| `crate_aliases` | map&lt;string, string&gt; | no | `{}` | Map dependency label, package name, or crate name to the local extern crate name |
| `aliases` | map&lt;string, string&gt; | no | `{}` | Bazel-compatible alias map from dependency label or crate name to local extern crate name |
| `named_deps` | map&lt;string, string&gt; | no | `{}` | Buck-compatible alias map from local extern crate name to dependency label or crate name |
| `cargo_package` | string | no | empty | Cargo package name used to select direct external deps from a `cargo_dependencies` dependency set |
| `build_script` | string | no | empty | Package-relative Cargo build script path run before `rustc` |
| `args` | list&lt;string&gt; | no | `[]` | Arguments passed to the compiled test binary |
| `test_env` | map&lt;string, string&gt; | no | `{}` | Environment variables passed to the test runner |
| `env_inherit` | list&lt;string&gt; | no | `[]` | Host environment variable names inherited by the test runner before `test_env` overrides |
| `crate` | target | no | empty | Reserved Bazel-compatible reference to an already-built crate under test |
| `use_libtest_harness` | bool | no | `true` | Whether to use the Rust libtest harness. Only `true` is supported |
| `labels` | list&lt;string&gt; | no | `[]` | Labels exposed through `once_test_info` for test discovery |
| `timeout_ms` | int | no |  | Optional test timeout in milliseconds |

Accepted but unsupported attributes: `default_deps`, `doc_deps`, `doc_env`, `doc_link_style`,
`doc_linker_flags`, `doc_named_deps`, `link_deps`, `link_style`,
`mapped_srcs`, `proc_macro_deps`, `rpath`, `runtime_dependency_handling`,
and `rustdoc_flags`. Non-empty values under `[target.attrs]` fail validation.
Use the dependency roles with the same names under `[target.dependencies]`.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `rust_crate`, `rust_proc_macro`, `rust_dependency_set`, `c_provider` | Rust crate dependencies consumed through `--extern`; C provider libraries and linker options are linked into the test executable |
| `proc_macro_deps` | `rust_proc_macro` | Procedural macros compiled for the execution host and passed to `rustc` through `--extern` |
| `link_deps` | `c_provider` | Native libraries and linker options consumed by the test executable |

## Providers

The target emits `rust_test` and `once_test_info`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `binary` |
| `test` | `default`, `test_results`, `logs` |

## Outputs

| Output | Location |
| --- | --- |
| Test binary | `.once/out/<target>/<crate_name>` or `.exe` on Windows |
| Test results | `.once/out/<target>/test/test_results.json` |
| Test log | `.once/out/<target>/test/rust-libtest.log` |
| Native runner output | `.once/out/<target>/test/native_results.txt` |

## Limitations

The `test` capability runs only host-target test binaries. A cross-target
`rust_test` can still be built, but running it needs a platform runner that is
not part of this target kind yet.

## Example

```toml
[[target]]
name = "hello"
kind = "rust_library"
srcs = ["src/**/*.rs"]

[target.attrs]
crate_name = "hello"
edition = "2021"

[[target]]
name = "hello_tests"
kind = "rust_test"
srcs = ["tests/**/*.rs"]
deps = ["./hello"]

[target.attrs]
crate_name = "hello_tests"
crate_root = "tests/greeting_test.rs"
edition = "2021"
labels = ["unit"]
```
