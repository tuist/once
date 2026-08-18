#shellcheck shell=bash
# End-to-end specs for the graph Once derives from a Cargo project without a
# manifest of its own. Each fixture is named after the open-source project
# whose layout it reproduces and stays small enough to compile quickly.

Describe 'cargo native project'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  cargo_toolchain_unavailable() {
    command -v cargo >/dev/null 2>&1 || return 0
    command -v rustc >/dev/null 2>&1 || return 0
    return 1
  }

  copy_fixture() {
    cp -R "$REPO_ROOT/fixtures/$1/." "$WORKSPACE/"
  }

  target_ids() {
    once --format json query targets | jq -r '.[] | "\(.kind) \(.id)"' | sort
  }

  test_summary() {
    jq -rc '"\(.summary.passed)/\(.summary.total) passed, \(.summary.failed) failed"' \
      "$WORKSPACE/.once/out/$1/test/test_results.json"
  }

  It 'derives a Cargo workspace graph with no once.toml present'
    cargo_toolchain_unavailable && Skip 'cargo and rustc are required'
    copy_fixture cargo_ripgrep

    When call target_ids
    The status should be success
    The stdout should include 'cargo_workspace cargo'
    The stdout should include 'rust_library cargo_grep'
    The stdout should include 'rust_binary cargo_ripgrep_bin_rg'
    The stdout should include 'rust_test cargo_ripgrep_test_integration'
    The path "$WORKSPACE/once.toml" should not be exist
  End

  It 'names a binary target after its Cargo target rather than its package'
    cargo_toolchain_unavailable && Skip 'cargo and rustc are required'
    copy_fixture cargo_fd

    When call target_ids
    The status should be success
    The stdout should include 'rust_binary cargo_fd_find_bin_fd'
    The stdout should include 'rust_test cargo_fd_find_test_tests'
  End

  It 'derives one test target per file under tests'
    cargo_toolchain_unavailable && Skip 'cargo and rustc are required'
    copy_fixture cargo_hyperfine

    When call target_ids
    The status should be success
    The stdout should include 'rust_test cargo_hyperfine_test_common'
    The stdout should include 'rust_test cargo_hyperfine_test_execution_order_tests'
    The stdout should include 'rust_test cargo_hyperfine_test_integration_tests'
  End

  It 'lowers a locked third-party package into the graph'
    cargo_toolchain_unavailable && Skip 'cargo and rustc are required'
    copy_fixture cargo_bat

    When call target_ids
    The status should be success
    The stdout should include 'rust_crate itoa-1.0.14'
    The stdout should include 'rust_library cargo_bat'
  End

  It 'builds and runs a binary whose sources sit outside src'
    cargo_toolchain_unavailable && Skip 'cargo and rustc are required'
    copy_fixture cargo_ripgrep
    once build cargo_ripgrep_bin_rg --quiet

    When call once run cargo_ripgrep_bin_rg -- needle
    The status should be success
    The stdout should be present
    The contents of file "$WORKSPACE/.once/out/cargo_ripgrep_bin_rg/run/stdout.log" should equal 'searching for needle'
  End

  It 'gives an integration test the package binary through CARGO_BIN_EXE'
    cargo_toolchain_unavailable && Skip 'cargo and rustc are required'
    copy_fixture cargo_ripgrep
    once test cargo_ripgrep_test_integration --quiet

    When call test_summary cargo_ripgrep_test_integration
    The status should be success
    The stdout should equal '1/1 passed, 0 failed'
  End

  It 'runs a test from its own package root'
    cargo_toolchain_unavailable && Skip 'cargo and rustc are required'
    copy_fixture cargo_ripgrep
    once test cargo_ignore_test_gitignore --quiet

    When call test_summary cargo_ignore_test_gitignore
    The status should be success
    The stdout should equal '1/1 passed, 0 failed'
  End

  It 'resolves an example against the development dependencies of its package'
    cargo_toolchain_unavailable && Skip 'cargo and rustc are required'
    copy_fixture cargo_ripgrep

    When call once build cargo_ignore_bin_walk --quiet
    The status should be success
    The stdout should be present
  End

  It 'gives a build script the environment Cargo documents'
    cargo_toolchain_unavailable && Skip 'cargo and rustc are required'
    copy_fixture cargo_mise
    once build cargo_mise_bin_mise --quiet

    When call once run cargo_mise_bin_mise
    The status should be success
    The stdout should be present
    The contents of file "$WORKSPACE/.once/out/cargo_mise_bin_mise/run/stdout.log" should equal 'aqua cache'
  End

  It 'compiles a workspace member used as a build dependency into one crate'
    cargo_toolchain_unavailable && Skip 'cargo and rustc are required'
    copy_fixture cargo_mise

    When call target_ids
    The status should be success
    The stdout should include 'rust_library cargo_mise_registry'
    The stdout should not include '-host'
  End

  It 'applies the Cargo configuration environment to the compiler and the test'
    cargo_toolchain_unavailable && Skip 'cargo and rustc are required'
    copy_fixture cargo_bat
    once test cargo_bat_test_integration_tests --quiet

    When call test_summary cargo_bat_test_integration_tests
    The status should be success
    The stdout should equal '3/3 passed, 0 failed'
  End

  It 'restores an unchanged Cargo build from the action cache'
    cargo_toolchain_unavailable && Skip 'cargo and rustc are required'
    copy_fixture cargo_fd
    once build cargo_fd_find_bin_fd --quiet

    When call once --format json build cargo_fd_find_bin_fd
    The status should be success
    The stdout should include '"cache":"hit"'
  End
End
