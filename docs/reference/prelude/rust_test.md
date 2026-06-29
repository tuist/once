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
| `rustc_flags` | list&lt;string&gt; | no | `[]` | Additional `rustc` flags appended after Once-managed flags |
| `cap_lints` | string | no | empty | Optional rustc lint cap passed as `--cap-lints` |
| `linker` | string | no | inferred | Optional linker path passed as `-C linker=...` |
| `linker_flags` | list&lt;string&gt; | no | `[]` | Additional linker flags lowered to `-C link-arg=...` |
| `crate_aliases` | map&lt;string, string&gt; | no | `{}` | Map dependency label, package name, or crate name to the local extern crate name |
| `cargo_package` | string | no | empty | Cargo package name used to select direct external deps from a `cargo_dependencies` dependency set |
| `build_script` | string | no | empty | Package-relative Cargo build script path run before `rustc` |
| `args` | list&lt;string&gt; | no | `[]` | Arguments passed to the compiled test binary |
| `test_env` | map&lt;string, string&gt; | no | `{}` | Environment variables passed to the test runner |
| `labels` | list&lt;string&gt; | no | `[]` | Labels exposed through `once_test_info` for test discovery |
| `timeout_ms` | int | no |  | Optional test timeout in milliseconds |

## Dep Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `rust_crate`, `rust_proc_macro`, `rust_dependency_set` | Rust crate dependencies consumed through `--extern` |

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
