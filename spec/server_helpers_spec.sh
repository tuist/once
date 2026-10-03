# shellcheck shell=bash

Describe 'test server readiness'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'accepts a populated readiness file'
    printf ready > "$WORKSPACE/ready"

    When call wait_for_server_file "$WORKSPACE/ready" "$$"
    The status should be success
    The stdout should be blank
    The stderr should be blank
  End

  It 'fails instead of polling forever after the server exits'
    "$SPEC_PYTHON3" -c 'pass' &
    server_pid=$!
    wait "$server_pid"

    When call wait_for_server_file "$WORKSPACE/ready" "$server_pid"
    The status should equal 1
    The stdout should be blank
    The stderr should include 'test server exited before creating'
  End

  It 'keeps Python available after isolating HOME'
    When call python3 -c 'print("ready")'
    The status should be success
    The stdout should equal 'ready'
    The stderr should be blank
  End
End
