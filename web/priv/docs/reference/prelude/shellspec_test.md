# `shellspec_test`

Runs ShellSpec files through Once's generic test capability and produces
normalized test results. Use it for shell-based end-to-end tests that belong
in the same test plan as compiled or language-native tests.

## Inputs and runner

Declare spec files with `srcs`. The `shellspec` executable defaults to
`shellspec` on the toolchain path. `args` supplies runner arguments, `env`
supplies declared environment, and `data` names additional input files.
Use `labels` for test metadata and `timeout_ms` to bound a run.

The `deps` role accepts `script_action`, `apple_linkable`, `apple_application`,
and `once_test_info` providers whose changes should affect the tests.

## Results

The target exposes `test` with `default`, `test_results`, and `logs` output
groups. The normalized result uses `once.test_results.v1`.

```sh
once query schema shellspec_test
once query example shellspec_test shellspec-test-minimal
```

Read [Testing and Scheduling](/guide/graph/testing) for selection, result
inspection, retries, and test-plan behavior.
