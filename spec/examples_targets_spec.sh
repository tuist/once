#shellcheck shell=bash
# End-to-end specs for listing checked-in example projects.

Describe 'examples target listing'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'lists checked-in Rust and Apple examples'
    copy_examples
    When call fabrik targets
    The status should be success
    The stdout should include 'examples/rust/granular/basic-app/greeting'
    The stdout should include 'examples/rust/granular/basic-app/hello'
    The stdout should include 'examples/rust/granular/basic-app/greeting_test'
    The stdout should include 'examples/apple/macos/cli/Greeter'
    The stdout should include 'examples/apple/macos/cli/hello'
    The stdout should include 'examples/apple/macos/tuist-shaped-swift/tuist'
    The stdout should include 'examples/apple/ios/simulator-app/Demo'
  End
End
