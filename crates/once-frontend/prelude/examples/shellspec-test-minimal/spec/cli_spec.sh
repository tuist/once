#shellcheck shell=sh

Describe 'example cli'
  Include './spec/spec_helper.sh'

  It 'prints a greeting'
    When call example_subject
    The output should equal 'hello once'
  End
End
