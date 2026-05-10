#shellcheck shell=bash
# End-to-end specs for checked-in Rust examples.

Describe 'Rust examples'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'builds and runs the checked-in Rust example'
    copy_examples
    fabrik build examples/rust/granular/basic-app/hello >/dev/null 2>&1
    When call "$WORKSPACE/.fabrik/out/examples/rust/granular/basic-app/hello"
    The status should be success
    The stdout should equal 'Hello, Rust, from Fabrik'
  End

  It 'reuses the cache for the checked-in Rust example build'
    copy_examples
    fabrik build examples/rust/granular/basic-app/hello >/dev/null 2>&1
    When call fabrik build examples/rust/granular/basic-app/hello
    The status should be success
    The stderr should include '2 nodes, 2 hit, 0 miss'
  End

  It 'builds and runs tests from the checked-in Rust example'
    copy_examples
    When call fabrik test examples/rust/granular/basic-app/greeting_test
    The status should be success
    The stdout should include 'test result: ok'
    The stderr should include 'build 2 nodes'
  End

  It 'reuses the cache for the checked-in Rust example tests'
    copy_examples
    fabrik test examples/rust/granular/basic-app/greeting_test >/dev/null 2>&1
    When call fabrik test examples/rust/granular/basic-app/greeting_test
    The status should be success
    The stdout should include 'test result: ok'
    The stderr should include 'test cache hit'
  End
End
