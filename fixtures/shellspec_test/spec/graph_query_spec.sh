#shellcheck shell=bash

Describe 'once graph query'
  Include './spec/spec_helper.sh'
  BeforeEach 'setup_nested_workspace'
  AfterEach 'cleanup_nested_workspace'

  It 'lists declared targets'
    {
      printf '%s\n' '[[target]]'
      printf '%s\n' 'name = "smoke"'
      printf '%s\n' 'kind = "shellspec_test"'
      printf '%s\n' 'srcs = ["spec/**/*_spec.sh"]'
    } > "$NESTED_WORKSPACE/once.toml"

    When call once_under_test query targets
    The status should be success
    The stdout should include 'smoke (shellspec_test) [test]'
  End
End
