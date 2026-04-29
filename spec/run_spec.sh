#shellcheck shell=bash
# End-to-end specs for `fabrik run`.

Describe 'fabrik run'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'executes the command and reports a cache miss on first run'
    When call fabrik run -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf hello'
    The status should be success
    The stdout should equal 'hello'
    The stderr should include 'cache miss'
    The stderr should include 'exit=0'
  End

  It 'returns the cached result on a second identical invocation'
    fabrik run -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf hi' >/dev/null 2>&1
    When call fabrik run -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf hi'
    The status should be success
    The stdout should equal 'hi'
    The stderr should include 'cache hit'
  End

  It 'does not re-execute the command on a cache hit'
    sentinel="$WORKSPACE/sentinel"
    cmd="touch '$sentinel'"
    fabrik run -e PATH=/usr/bin:/bin -- /bin/sh -c "$cmd" >/dev/null 2>&1
    rm -f "$sentinel"
    When call fabrik run -e PATH=/usr/bin:/bin -- /bin/sh -c "$cmd"
    The status should be success
    The stderr should include 'cache hit'
    The path "$sentinel" should not be exist
  End

  It 'propagates the underlying exit code'
    When call fabrik run -e PATH=/usr/bin:/bin -- /bin/sh -c 'exit 7'
    The status should equal 7
    The stderr should include 'exit=7'
  End

  It 'caches non-zero exits like any other result'
    fabrik run -e PATH=/usr/bin:/bin -- /bin/sh -c 'exit 3' >/dev/null 2>&1 || true
    When call fabrik run -e PATH=/usr/bin:/bin -- /bin/sh -c 'exit 3'
    The status should equal 3
    The stderr should include 'cache hit'
  End

  It 'uses argv as part of the cache key'
    fabrik run -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf one' >/dev/null 2>&1
    When call fabrik run -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf two'
    The status should be success
    The stdout should equal 'two'
    The stderr should include 'cache miss'
  End

  It 'uses env as part of the cache key'
    fabrik run -e PATH=/usr/bin:/bin -e MODE=a -- /bin/sh -c 'printf $MODE' >/dev/null 2>&1
    When call fabrik run -e PATH=/usr/bin:/bin -e MODE=b -- /bin/sh -c 'printf $MODE'
    The status should be success
    The stdout should equal 'b'
    The stderr should include 'cache miss'
  End

  It 'starts subprocesses with a cleared environment'
    # FABRIK_PROBE is set in the parent shell but not declared via -e, so
    # the child must not see it. The exit code distinguishes the cases:
    # 0 = unset (good), 1 = leaked (bad).
    export FABRIK_PROBE=leaked
    When call fabrik run -e PATH=/usr/bin:/bin -- /bin/sh -c '[ -z "${FABRIK_PROBE:-}" ]'
    The status should be success
    The stderr should include 'exit=0'
  End

  It 'creates the .fabrik directory inside the workspace'
    fabrik run -e PATH=/usr/bin:/bin -- /bin/sh -c 'true' >/dev/null 2>&1
    When call test -d "$WORKSPACE/.fabrik/cas"
    The status should be success
  End
End
