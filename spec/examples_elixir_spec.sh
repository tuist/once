#shellcheck shell=bash
# End-to-end specs for checked-in Elixir examples.
#
# The host doesn't need a real Elixir toolchain: a small `elixirc` shim
# in $WORKSPACE/bin writes one fake `.beam` per source so we can exercise
# the cache + output capture contract without pulling OTP into CI.

fabrik_with_fake_elixir() {
  PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" "$@"
}

Describe 'Elixir examples'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'builds the checked-in Elixir library example and produces .beam files'
    copy_examples
    fake_elixir_tools
    When call fabrik_with_fake_elixir build examples/elixir/granular/basic-app/greeting
    The status should be success
    The stderr should include '1 nodes, 0 hit, 1 miss'
    The path "$WORKSPACE/.fabrik/out/examples/elixir/granular/basic-app/greeting.ebin" should be directory
    The path "$WORKSPACE/.fabrik/out/examples/elixir/granular/basic-app/greeting.ebin/Elixir.Greeting.beam" should be file
  End

  It 'reuses the cache for the checked-in Elixir library example'
    copy_examples
    fake_elixir_tools
    fabrik_with_fake_elixir build examples/elixir/granular/basic-app/greeting >/dev/null 2>&1
    When call fabrik_with_fake_elixir build examples/elixir/granular/basic-app/greeting
    The status should be success
    The stderr should include '1 nodes, 1 hit, 0 miss'
  End

  It 'builds the checked-in Elixir binary example with a launcher and ebin'
    copy_examples
    fake_elixir_tools
    When call fabrik_with_fake_elixir build examples/elixir/granular/basic-app/hello
    The status should be success
    The stderr should include '2 nodes, 0 hit, 2 miss'
    The path "$WORKSPACE/.fabrik/out/examples/elixir/granular/basic-app/hello" should be file
    The path "$WORKSPACE/.fabrik/out/examples/elixir/granular/basic-app/hello.ebin" should be directory
    The contents of file "$WORKSPACE/.fabrik/out/examples/elixir/granular/basic-app/hello" should include 'Hello.main(System.argv())'
    The contents of file "$WORKSPACE/.fabrik/out/examples/elixir/granular/basic-app/hello" should include 'greeting.ebin'
  End
End
