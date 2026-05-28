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

  It 'hashes bytes with the same digest as blob put'
    hash_digest="$(printf 'hello cache' | fabrik cache hash)"
    blob_digest="$(printf 'hello cache' | fabrik cache blob put)"

    When call /bin/sh -c '[ "$1" = "$2" ]' -- "$hash_digest" "$blob_digest"
    The status should be success
  End

  It 'does not store hashed bytes'
    printf 'hello cache' | fabrik cache hash >/dev/null
    When call fabrik cache stats
    The status should be success
    The stdout should include 'blobs:   0'
  End

  It 'combines digests in a stable ordered form'
    one="$(printf 'one' | fabrik cache hash)"
    two="$(printf 'two' | fabrik cache hash)"
    combined="$(fabrik cache hash --combine "$one" "$two")"
    same="$(fabrik cache hash --combine "$one" "$two")"
    reverse="$(fabrik cache hash --combine "$two" "$one")"

    When call /bin/sh -c '[ "$1" = "$2" ] && [ "$1" != "$3" ]' -- "$combined" "$same" "$reverse"
    The status should be success
  End

  It 'emits structured hash metadata'
    When call "$FABRIK_BIN" --format json -C "$WORKSPACE" cache hash --combine \
      3ef81072da29fe9d6c28c389fd497c20e8ffde86fd335b326c34a9d7a56f0dbc
    The status should be success
    The stdout should include '"mode":"combine"'
    The stdout should include '"parts":1'
    The stdout should include '"digest":'
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
