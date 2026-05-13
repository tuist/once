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
    The stdout should include 'test'
    The stdout should include 'exec'
    The stdout should include 'cache'
    The stdout should include 'deps'
    The stdout should include 'runtime'
    The stdout should include '--directory'
    The stdout should include 'toon'
  End
End

Describe 'fabrik --version'
  It 'prints the version'
    When call "$FABRIK_BIN" --version
    The status should be success
    The stdout should include 'fabrik'
  End
End

Describe 'fabrik exec -e'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'rejects a malformed env entry without an equals sign'
    When call fabrik exec -e MALFORMED -- /bin/sh -c 'true'
    The status should not equal 0
    The stderr should include 'KEY=VALUE'
  End

  It 'accepts an empty value (KEY=)'
    When call fabrik exec -e PATH=/usr/bin:/bin -e EMPTY= -- /bin/sh -c 'printf "[$EMPTY]"'
    The status should be success
    The stdout should equal '[]'
    The stderr should include 'cache miss'
  End

  It 'preserves equals signs inside the value'
    When call fabrik exec -e PATH=/usr/bin:/bin -e PAIR=a=b -- /bin/sh -c 'printf "$PAIR"'
    The status should be success
    The stdout should equal 'a=b'
    The stderr should include 'cache miss'
  End
End

Describe 'fabrik -C <directory>'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'populates the CAS under XDG_CACHE_HOME on first use'
    fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'true' >/dev/null 2>&1
    When call test -d "$XDG_CACHE_HOME/fabrik/cas"
    The status should be success
  End

  It 'errors clearly when given a non-directory path'
    # `setup_workspace` ensures any incidental writes still land under
    # the per-test XDG roots even though the dispatch check bails
    # before the CAS is opened.
    file="$WORKSPACE/not-a-directory"
    : > "$file"
    When call "$FABRIK_BIN" -C "$file" exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'true'
    The status should not equal 0
    The stderr should include 'fabrik:'
  End
End

Describe 'fabrik exec output'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'prints the action digest as 64 hex characters'
    When call fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'true'
    The status should be success
    The stderr should match pattern '*action=????????????????????????????????????????????????????????????????*'
  End

  # Binary-safe stdout (null bytes) is covered by a Rust unit test in
  # fabrik-core where the assertion can inspect the raw blob bytes
  # directly. Shellspec's pipeline machinery isn't a reliable carrier
  # for null-bearing output across shells.
End

Describe 'fabrik cache stats with a fresh XDG cache'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  # `setup_workspace` provisions a fresh `$XDG_CACHE_HOME` per test, so
  # the CAS starts empty and `cache stats` reports zero across the
  # board regardless of any cache state on the developer's machine.
  It 'reports zero blobs and zero actions before anything has run'
    When call fabrik cache stats
    The status should be success
    The stdout should include 'blobs:   0'
    The stdout should include 'actions: 0'
  End
End

Describe 'sequential invocations against the same workspace'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  # Concurrent in-process safety is covered by Rust integration tests
  # (`many_independent_actions_execute_concurrently`,
  # `concurrent_writers_of_identical_blob_do_not_race`) which can
  # inspect Runner state directly. This spec just confirms the CLI's
  # external contract: back-to-back invocations against the same
  # workspace each see their own results.
  It 'two back-to-back execs return distinct outputs'
    fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf one' >/dev/null 2>&1
    When call fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf two'
    The status should be success
    The stdout should equal 'two'
    The stderr should include 'cache miss'
  End
End
