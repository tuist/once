#shellcheck shell=bash
# End-to-end specs for `struo cache gc`.

Describe 'struo cache gc'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'is a no-op on an empty workspace'
    When call struo cache gc --max-size 1GB
    The status should be success
    The stdout should include '0 entries removed'
  End

  It 'leaves the cache intact when it already fits the budget'
    struo exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf hello' >/dev/null 2>&1
    When call struo cache gc --max-size 1GiB
    The status should be success
    The stdout should include 'reclaimed 0 bytes'
  End

  It 'reclaims the whole store under a zero budget'
    struo exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf hello' >/dev/null 2>&1
    struo cache gc --max-size 0 >/dev/null 2>&1
    # After a full reclaim the store reports zero blobs and actions.
    When call struo cache stats
    The status should be success
    The stdout should include 'blobs:   0'
    The stdout should include 'actions: 0'
  End

  It 'previews reclaimable space with --dry-run without deleting'
    struo exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf hello' >/dev/null 2>&1
    struo cache gc --max-size 0 --dry-run >/dev/null 2>&1
    # The store is untouched: blobs and the action are still present.
    When call struo cache stats
    The status should be success
    The stdout should not include 'blobs:   0'
  End

  It 'rejects a malformed size'
    When call struo cache gc --max-size not-a-size
    The status should not equal 0
    The stderr should include 'max-size'
  End

  It 'emits a structured record under --format json'
    When call "$STRUO_BIN" --format json -C "$WORKSPACE" cache gc --max-size 500MB
    The status should be success
    The stdout should include '"bytes_reclaimed":'
    The stdout should include '"removed":'
    The stdout should include '"max_size":500000000'
  End
End
