#shellcheck shell=bash
# End-to-end specs for the graph Once derives from an Nx workspace without a
# manifest of its own. `nx graph --print` returns the project graph that Once
# translates into `nx_task` targets so it can schedule and cache them.

Describe 'nx native project'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  nx_toolchain_unavailable() {
    command -v node >/dev/null 2>&1 || return 0
    command -v npm >/dev/null 2>&1 || return 0
    return 1
  }

  copy_fixture() {
    cp -R "$REPO_ROOT/fixtures/$1/." "$WORKSPACE/"
  }

  install_fixture() {
    (cd "$WORKSPACE" && npm install --no-audit --no-fund --loglevel=error >/dev/null 2>&1)
  }

  target_ids() {
    once --format json query targets | jq -r '.[] | "\(.kind) \(.id)"' | sort
  }

  It 'derives Nx tasks with no once.toml present'
    nx_toolchain_unavailable && Skip 'node and npm are required'
    copy_fixture nx_hello
    install_fixture || Skip 'npm install failed; skipping (offline?)'

    When call target_ids
    The status should be success
    The stdout should include 'nx_workspace nx'
    The stdout should include 'nx_task hello_world__build'
    The stdout should include 'nx_task hello_world__test'
    The stdout should include 'nx_task greeter__build'
    The path "$WORKSPACE/once.toml" should not be exist
  End
End
