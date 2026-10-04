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

  assert_server_closes_harness_pipe() {
    mkfifo "$WORKSPACE/harness-pipe"
    (
      cat "$WORKSPACE/harness-pipe" > "$WORKSPACE/pipe-output"
      printf done > "$WORKSPACE/reader-done"
    ) &
    pipe_test_reader_pid=$!
    exec 42>"$WORKSPACE/harness-pipe"
    start_python_server "$REPO_ROOT/fixtures/tool_graph/http_server.py" \
      "$WORKSPACE" "$WORKSPACE/server-port" > "$WORKSPACE/server.log" 2>&1 &
    pipe_test_server_pid=$!
    exec 42>&-

    pipe_test_status=0
    wait_for_server_file "$WORKSPACE/server-port" "$pipe_test_server_pid" &&
      wait_for_server_file "$WORKSPACE/reader-done" "$pipe_test_reader_pid" &&
      kill -0 "$pipe_test_server_pid" &&
      [ ! -s "$WORKSPACE/pipe-output" ] || pipe_test_status=1

    kill "$pipe_test_server_pid" "$pipe_test_reader_pid" 2>/dev/null || :
    wait "$pipe_test_server_pid" 2>/dev/null || :
    wait "$pipe_test_reader_pid" 2>/dev/null || :
    return "$pipe_test_status"
  }

  assert_server_does_not_resolve_loopback() {
    cat > "$WORKSPACE/sitecustomize.py" <<'PY'
import socket

def unavailable_reverse_dns(*args, **kwargs):
    raise RuntimeError("reverse DNS is unavailable")

socket.getfqdn = unavailable_reverse_dns
PY
    PYTHONPATH="$WORKSPACE" start_python_server \
      "$REPO_ROOT/fixtures/tool_graph/http_server.py" \
      "$WORKSPACE" "$WORKSPACE/server-port" > "$WORKSPACE/server.log" 2>&1 &
    loopback_server_pid=$!
    loopback_status=0
    wait_for_server_file "$WORKSPACE/server-port" "$loopback_server_pid" || loopback_status=1
    kill "$loopback_server_pid" 2>/dev/null || :
    wait "$loopback_server_pid" 2>/dev/null || :
    return "$loopback_status"
  }

  It 'starts a loopback fixture server without reverse DNS'
    When call assert_server_does_not_resolve_loopback
    The status should be success
    The stdout should be blank
    The stderr should be blank
    The path "$WORKSPACE/server-port" should be file
    The contents of file "$WORKSPACE/server.log" should be blank
  End

  It 'closes inherited harness pipes before starting a fixture server'
    When call assert_server_closes_harness_pipe
    The status should be success
    The stdout should be blank
    The stderr should be blank
  End
End
