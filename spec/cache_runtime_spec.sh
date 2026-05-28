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

  It 'hashes multiple files as if combined'
    printf 'package' > "$WORKSPACE/pkg.json"
    printf 'lock' > "$WORKSPACE/lock.json"
    one="$(fabrik cache hash "$WORKSPACE/pkg.json")"
    two="$(fabrik cache hash "$WORKSPACE/lock.json")"
    expected="$(fabrik cache hash --combine "$one" "$two")"
    multi="$(fabrik cache hash "$WORKSPACE/pkg.json" "$WORKSPACE/lock.json")"

    When call /bin/sh -c '[ "$1" = "$2" ]' -- "$expected" "$multi"
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

  It 'exits 0 when blob exists'
    digest="$(printf 'present' | fabrik cache blob put)"
    When call fabrik cache blob exists "$digest"
    The status should be success
  End

  It 'exits non-zero when blob is missing'
    When call fabrik cache blob exists 0000000000000000000000000000000000000000000000000000000000000000
    The status should be failure
  End

  It 'reports blob existence as JSON without erroring on miss'
    When call "$FABRIK_BIN" --format json -C "$WORKSPACE" cache blob exists \
      0000000000000000000000000000000000000000000000000000000000000000
    The status should be success
    The stdout should include '"present":false'
  End

  It 'stores a blob under a caller-chosen key and retrieves it'
    key="$(printf 'inputs' | fabrik cache hash)"
    printf 'artifact bytes' | fabrik cache blob put --key "$key" >/dev/null
    When call fabrik cache blob get --key "$key"
    The status should be success
    The stdout should equal 'artifact bytes'
  End

  It 'reports a keyed blob as present via exists --key'
    key="$(printf 'inputs' | fabrik cache hash)"
    printf 'artifact bytes' | fabrik cache blob put --key "$key" >/dev/null
    When call fabrik cache blob exists --key "$key"
    The status should be success
  End

  It 'does not expose keyed blobs via the content-addressed namespace'
    key="$(printf 'inputs' | fabrik cache hash)"
    printf 'artifact bytes' | fabrik cache blob put --key "$key" >/dev/null
    When call fabrik cache blob exists "$key"
    The status should be failure
  End

  It 'rejects duplicate `-` in cache hash'
    When call fabrik cache hash - -
    The status should be failure
    The stderr should include 'may appear at most once'
  End

  It 'reports separate keyed counts in cache stats'
    printf 'cas-bytes' | fabrik cache blob put >/dev/null
    key="$(printf 'k' | fabrik cache hash)"
    printf 'keyed-bytes' | fabrik cache blob put --key "$key" >/dev/null
    When call fabrik cache stats
    The status should be success
    The stdout should include 'blobs:   1'
    The stdout should include 'keyed:   1'
  End
End
