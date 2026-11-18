#shellcheck shell=bash

Describe 'struo cli surface'
  Include './spec/spec_helper.sh'
  BeforeEach 'setup_nested_workspace'
  AfterEach 'cleanup_nested_workspace'

  It 'lists commands'
    When call struo_under_test --list
    The status should be success
    The stdout should include 'test'
    The stdout should include 'query'
  End
End
