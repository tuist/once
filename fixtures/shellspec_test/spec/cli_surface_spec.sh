#shellcheck shell=bash

Describe 'once cli surface'
  Include './spec/spec_helper.sh'
  BeforeEach 'setup_nested_workspace'
  AfterEach 'cleanup_nested_workspace'

  It 'lists commands'
    When call once_under_test --list
    The status should be success
    The stdout should include 'test'
    The stdout should include 'query'
  End
End
