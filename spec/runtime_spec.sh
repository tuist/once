#shellcheck shell=bash
# End-to-end specs for persisted runtime sessions.

Describe 'once runtime'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  seed_echo_target() {
    cat > "$WORKSPACE/once.toml" <<'EOF'
[[target]]
name = "Echo"
kind = "task"

[target.attrs]
argv_json = "[\"/bin/sh\",\"-c\",\"printf out; printf err >&2\"]"
cache = "false"
EOF
  }

  It 'starts a target session and reads persisted logs'
    seed_echo_target

    result="$(once --format json runtime start Echo)"
    session_id="$(
      printf '%s' "$result" | python3 -c 'import json, sys; print(json.load(sys.stdin)["session_id"])'
    )"

    for _ in $(seq 1 50); do
      status_json="$(once --format json runtime status "$session_id")"
      case "$status_json" in
        *'"status":"exited"'*|*'"status":"failed"'*) break ;;
      esac
      sleep 0.1
    done

    status_json="$(once --format json runtime status "$session_id")"
    stdout_logs="$(once --format json runtime logs "$session_id" --source stdout)"
    stderr_logs="$(once --format json runtime logs "$session_id" --source stderr)"

    The variable status_json should include '"status":"exited"'
    The variable status_json should include '"exit_code":0'
    The variable stdout_logs should include '"source":"stdout"'
    The variable stdout_logs should include '"message":"out"'
    The variable stdout_logs should not include '"source":"stderr"'
    The variable stderr_logs should include '"source":"stderr"'
    The variable stderr_logs should include '"message":"err"'
    The variable stderr_logs should not include '"source":"stdout"'
  End
End
