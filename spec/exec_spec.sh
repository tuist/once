#shellcheck shell=bash
# End-to-end specs for `fabrik exec` (substrate escape hatch).

Describe 'fabrik exec'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'executes the command and reports a cache miss on first run'
    When call fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf hello'
    The status should be success
    The stdout should equal 'hello'
    The stderr should include 'cache miss'
    The stderr should include 'exit=0'
  End

  It 'returns the cached result on a second identical invocation'
    fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf hi' >/dev/null 2>&1
    When call fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf hi'
    The status should be success
    The stdout should equal 'hi'
    The stderr should include 'cache hit'
  End

  It 'does not re-execute the command on a cache hit'
    sentinel="$WORKSPACE/sentinel"
    cmd="touch '$sentinel'"
    fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c "$cmd" >/dev/null 2>&1
    rm -f "$sentinel"
    When call fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c "$cmd"
    The status should be success
    The stderr should include 'cache hit'
    The path "$sentinel" should not be exist
  End

  It 'propagates the underlying exit code'
    When call fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'exit 7'
    The status should equal 7
    The stderr should include 'exit=7'
  End

  It 'does not cache failing exits by default'
    fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'exit 3' >/dev/null 2>&1 || true
    When call fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'exit 3'
    The status should equal 3
    # Second run is still a miss; failures are intentionally not
    # written to the cache without --cache-failures.
    The stderr should include 'cache miss'
  End

  It 'caches failing exits when --cache-failures is set'
    fabrik exec --cache-failures -e PATH=/usr/bin:/bin -- /bin/sh -c 'exit 3' >/dev/null 2>&1 || true
    When call fabrik exec --cache-failures -e PATH=/usr/bin:/bin -- /bin/sh -c 'exit 3'
    The status should equal 3
    The stderr should include 'cache hit'
  End

  It 'uses argv as part of the cache key'
    fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf one' >/dev/null 2>&1
    When call fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf two'
    The status should be success
    The stdout should equal 'two'
    The stderr should include 'cache miss'
  End

  It 'uses env as part of the cache key'
    # printenv reads the env directly without going through a shell;
    # no quoting variability across bash/dash on different runners.
    fabrik exec -e PATH=/usr/bin:/bin -e MODE=a -- /usr/bin/printenv MODE >/dev/null 2>&1
    When call fabrik exec -e PATH=/usr/bin:/bin -e MODE=b -- /usr/bin/printenv MODE
    The status should be success
    The stdout should match pattern 'b*'
    The stderr should include 'cache miss'
  End

  It 'starts subprocesses with a cleared environment'
    export FABRIK_PROBE=leaked
    When call fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c '[ -z "${FABRIK_PROBE:-}" ]'
    The status should be success
    The stderr should include 'exit=0'
  End

  It 'creates the .fabrik directory inside the workspace'
    fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'true' >/dev/null 2>&1
    When call test -d "$WORKSPACE/.fabrik/cas"
    The status should be success
  End

  It 'reports the cache hit in well under 100ms'
    # Warm the cache with a deliberately slow command.
    fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'sleep 0.3' >/dev/null 2>&1
    start_ns=$(date +%s%N 2>/dev/null || python3 -c 'import time; print(int(time.time()*1e9))')
    fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'sleep 0.3' >/dev/null 2>&1
    end_ns=$(date +%s%N 2>/dev/null || python3 -c 'import time; print(int(time.time()*1e9))')
    elapsed_ms=$(( (end_ns - start_ns) / 1000000 ))
    When call test "$elapsed_ms" -lt 100
    The status should be success
  End

  Describe 'cwd'
    It 'resolves a workspace-relative cwd against the workspace root'
      # Use `pwd` (POSIX shell builtin) so the assertion checks the
      # cwd resolution itself, without depending on file-write timing
      # or external utilities discoverable through PATH.
      mkdir -p "$WORKSPACE/sub"
      When call fabrik exec --cwd sub -e PATH=/usr/bin:/bin -- /bin/sh -c 'pwd'
      The status should be success
      The stdout should match pattern '*/sub*'
      The stderr should include 'cache miss'
    End

    It 'rejects an absolute cwd'
      When call fabrik exec --cwd /tmp -e PATH=/usr/bin:/bin -- /bin/sh -c 'true'
      The status should not equal 0
      The stderr should include 'workspace path must be relative'
    End

    It 'rejects a parent-directory escape'
      When call fabrik exec --cwd ../escape -e PATH=/usr/bin:/bin -- /bin/sh -c 'true'
      The status should not equal 0
      The stderr should include 'must not escape'
    End
  End

  Describe '--timeout-ms'
    It 'kills the child if the deadline elapses'
      When call fabrik exec --timeout-ms 100 -e PATH=/usr/bin:/bin -- /bin/sh -c 'sleep 5'
      The status should not equal 0
      The stderr should include 'timeout'
    End
  End

  Describe 'large outputs'
    big_exec() {
      "$FABRIK_BIN" -C "$WORKSPACE" exec --timeout-ms 10000 \
        -e PATH=/usr/bin:/bin -- /bin/sh -c 'yes hello | head -c 4194304' \
        > "$WORKSPACE/big.out" 2>/dev/null
      wc -c "$WORKSPACE/big.out" | awk '{print $1}'
    }

    It 'streams multi-megabyte output without buffering everything'
      # Run via a helper so the 4 MB stream stays in a real file and
      # shellspec only sees the byte count on stdout.
      When call big_exec
      The status should be success
      The output should equal '4194304'
    End
  End
End
