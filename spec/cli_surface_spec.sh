#shellcheck shell=bash
# CLI surface checks: argument parsing, help, version, error messages.
# Each example owns its workspace where applicable; specs without state
# don't need one.

Describe 'fabrik --help'
  It 'prints usage and exits 0'
    When call "$FABRIK_BIN" --help
    The status should be success
    The stdout should include 'fabrik'
    The stdout should include 'run'
    The stdout should include 'cache'
    The stdout should include '--directory'
  End
End

Describe 'fabrik --version'
  It 'prints the version'
    When call "$FABRIK_BIN" --version
    The status should be success
    The stdout should include 'fabrik'
  End
End

Describe 'fabrik run -e'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'rejects a malformed env entry without an equals sign'
    When call fabrik run -e MALFORMED -- /bin/sh -c 'true'
    The status should not equal 0
    The stderr should include 'KEY=VALUE'
  End

  It 'accepts an empty value (KEY=)'
    When call fabrik run -e PATH=/usr/bin:/bin -e EMPTY= -- /bin/sh -c 'printf "[$EMPTY]"'
    The status should be success
    The stdout should equal '[]'
    The stderr should include 'cache miss'
  End

  It 'preserves equals signs inside the value'
    When call fabrik run -e PATH=/usr/bin:/bin -e PAIR=a=b -- /bin/sh -c 'printf "$PAIR"'
    The status should be success
    The stdout should equal 'a=b'
    The stderr should include 'cache miss'
  End
End

Describe 'fabrik -C <directory>'
  It 'creates the .fabrik tree under a previously empty directory'
    fresh="$(mktemp -d -t fabrik-cli-fresh.XXXXXX)"
    "$FABRIK_BIN" -C "$fresh" run -e PATH=/usr/bin:/bin -- /bin/sh -c 'true' >/dev/null 2>&1
    rc=$?
    test -d "$fresh/.fabrik/cas" && test -d "$fresh/.fabrik/actions"
    When call test $rc -eq 0
    The status should be success
    rm -rf "$fresh"
  End

  It 'errors clearly when given a non-directory path'
    file="$(mktemp -t fabrik-cli-notdir.XXXXXX)"
    When call "$FABRIK_BIN" -C "$file" run -e PATH=/usr/bin:/bin -- /bin/sh -c 'true'
    The status should not equal 0
    The stderr should include 'fabrik:'
    rm -f "$file"
  End
End

Describe 'fabrik run output'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'prints the action digest as 64 hex characters'
    When call fabrik run -e PATH=/usr/bin:/bin -- /bin/sh -c 'true'
    The status should be success
    The stderr should match pattern '*action=????????????????????????????????????????????????????????????????*'
  End

  It 'survives binary stdout (null bytes)'
    bin_out() {
      "$FABRIK_BIN" -C "$WORKSPACE" run -e PATH=/usr/bin:/bin \
        -- /bin/sh -c 'printf "abc\000def"' > "$WORKSPACE/out.bin" 2>/dev/null
      wc -c "$WORKSPACE/out.bin" | awk '{print $1}'
    }
    When call bin_out
    The status should be success
    The output should equal '7'
  End
End

Describe 'fabrik cache stats with -C'
  It 'reports stats from a workspace it never wrote to'
    fresh="$(mktemp -d -t fabrik-stats.XXXXXX)"
    When call "$FABRIK_BIN" -C "$fresh" cache stats
    The status should be success
    The stdout should include 'blobs:   0'
    The stdout should include 'actions: 0'
    rm -rf "$fresh"
  End
End

Describe 'parallel invocations against the same workspace'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'two concurrent miss-then-hit runs both succeed'
    # Two distinct actions so neither gets a cache hit; if the CAS
    # races on temp files they'd both fail.
    parallel_runs() {
      "$FABRIK_BIN" -C "$WORKSPACE" run -e PATH=/usr/bin:/bin \
        -- /bin/sh -c 'sleep 0.1; printf one' > "$WORKSPACE/a.out" 2>/dev/null &
      pid_a=$!
      "$FABRIK_BIN" -C "$WORKSPACE" run -e PATH=/usr/bin:/bin \
        -- /bin/sh -c 'sleep 0.1; printf two' > "$WORKSPACE/b.out" 2>/dev/null &
      pid_b=$!
      wait $pid_a; rc_a=$?
      wait $pid_b; rc_b=$?
      printf '%s/%s/%s/%s' "$rc_a" "$rc_b" "$(cat "$WORKSPACE/a.out")" "$(cat "$WORKSPACE/b.out")"
    }
    When call parallel_runs
    The status should be success
    The output should equal '0/0/one/two'
  End

  It 'two concurrent runs of the SAME action both succeed'
    # Same digest → both compete to write the same cache entry. The
    # CAS uniqueness scheme (PID + counter) plus rename-then-skip on
    # already-exists must keep both returning success.
    same_runs() {
      "$FABRIK_BIN" -C "$WORKSPACE" run -e PATH=/usr/bin:/bin \
        -- /bin/sh -c 'sleep 0.1; printf shared' > "$WORKSPACE/a.out" 2>/dev/null &
      pid_a=$!
      "$FABRIK_BIN" -C "$WORKSPACE" run -e PATH=/usr/bin:/bin \
        -- /bin/sh -c 'sleep 0.1; printf shared' > "$WORKSPACE/b.out" 2>/dev/null &
      pid_b=$!
      wait $pid_a; rc_a=$?
      wait $pid_b; rc_b=$?
      printf '%s/%s/%s/%s' "$rc_a" "$rc_b" "$(cat "$WORKSPACE/a.out")" "$(cat "$WORKSPACE/b.out")"
    }
    When call same_runs
    The status should be success
    The output should equal '0/0/shared/shared'
  End
End
