#shellcheck shell=bash
# End-to-end specs for checked-in Elixir examples.
#
# These specs require a real Elixir + Erlang/OTP toolchain on PATH;
# they're pinned in the repo's mise.toml so `mise exec -- shellspec`
# picks them up automatically.

fabrik_with_elixir() {
  # Workspace bin (the fabrik symlink) wins, then mise's elixir + elixirc
  # come from the outer PATH inherited via `mise exec`.
  PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" "$@"
}

Describe 'Elixir examples'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'builds the checked-in Elixir library example and produces a real .beam'
    copy_examples
    stage_elixir_workspace_bin
    When call fabrik_with_elixir build examples/elixir/granular/basic-app/greeting
    The status should be success
    The stderr should include '1 nodes, 0 hit, 1 miss'
    The path "$WORKSPACE/.fabrik/out/examples/elixir/granular/basic-app/greeting.ebin" should be directory
    The path "$WORKSPACE/.fabrik/out/examples/elixir/granular/basic-app/greeting.ebin/Elixir.Greeting.beam" should be file
  End

  It 'reuses the cache for the checked-in Elixir library example'
    copy_examples
    stage_elixir_workspace_bin
    fabrik_with_elixir build examples/elixir/granular/basic-app/greeting >/dev/null 2>&1
    When call fabrik_with_elixir build examples/elixir/granular/basic-app/greeting
    The status should be success
    The stderr should include '1 nodes, 1 hit, 0 miss'
  End

  It 'builds the checked-in Elixir binary example with a launcher and ebin'
    copy_examples
    stage_elixir_workspace_bin
    When call fabrik_with_elixir build examples/elixir/granular/basic-app/hello
    The status should be success
    The stderr should include '2 nodes, 0 hit, 2 miss'
    The path "$WORKSPACE/.fabrik/out/examples/elixir/granular/basic-app/hello" should be file
    The path "$WORKSPACE/.fabrik/out/examples/elixir/granular/basic-app/hello.ebin" should be directory
    The contents of file "$WORKSPACE/.fabrik/out/examples/elixir/granular/basic-app/hello" should include 'Hello.main(System.argv())'
    The contents of file "$WORKSPACE/.fabrik/out/examples/elixir/granular/basic-app/hello" should include 'greeting.ebin'
  End

  It 'runs the produced launcher and prints the greeting'
    copy_examples
    stage_elixir_workspace_bin
    fabrik_with_elixir build examples/elixir/granular/basic-app/hello >/dev/null 2>&1
    When call "$WORKSPACE/.fabrik/out/examples/elixir/granular/basic-app/hello"
    The status should be success
    The stdout should equal 'Hello, Elixir, from Fabrik'
  End
End
