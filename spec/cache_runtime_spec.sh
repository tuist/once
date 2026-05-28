#shellcheck shell=bash
# End-to-end specs for direct cache access commands.

Describe 'fabrik cache runtime commands'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'stores and fetches a blob from stdin'
    digest="$(printf 'hello cache' | fabrik cache blob put)"
    When call fabrik cache blob get "$digest"
    The status should be success
    The stdout should equal 'hello cache'
  End

  It 'stores a file blob and writes it back to an output path'
    printf 'file payload' > "$WORKSPACE/input.txt"
    digest="$(fabrik cache blob put "$WORKSPACE/input.txt")"
    When call fabrik cache blob get "$digest" --output "$WORKSPACE/out/nested.txt"
    The status should be success
    The stdout should include 'wrote 12 bytes'
    The contents of file "$WORKSPACE/out/nested.txt" should equal 'file payload'
  End

  It 'stores and fetches an action result'
    stdout_digest="$(printf 'out' | fabrik cache blob put)"
    stderr_digest="$(printf 'err' | fabrik cache blob put)"
    action_digest="$stdout_digest"

    fabrik cache action put "$action_digest" \
      --exit-code 7 \
      --stdout "$stdout_digest" \
      --stderr "$stderr_digest" \
      --output bin/tool="$stdout_digest" >/dev/null

    When call fabrik cache action get "$action_digest"
    The status should be success
    The stdout should include '"exit_code": 7'
    The stdout should include '"stdout": "'$stdout_digest'"'
    The stdout should include '"bin/tool": "'$stdout_digest'"'
  End

  It 'forgets an action result'
    stdout_digest="$(printf 'out' | fabrik cache blob put)"
    stderr_digest="$(printf 'err' | fabrik cache blob put)"
    action_digest="$stdout_digest"

    fabrik cache action put "$action_digest" \
      --exit-code 7 \
      --stdout "$stdout_digest" \
      --stderr "$stderr_digest" >/dev/null

    When call fabrik cache action forget "$action_digest"
    The status should be success
    The stdout should include 'forgot action'
  End

  It 'reports an action miss as JSON'
    stdout_digest="$(printf 'out' | fabrik cache blob put)"
    action_digest="$stdout_digest"

    When call "$FABRIK_BIN" --format json -C "$WORKSPACE" cache action get "$action_digest"
    The status should be success
    The stdout should include '"hit":false'
  End
End
