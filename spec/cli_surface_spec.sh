#shellcheck shell=bash
# CLI surface checks: argument parsing, help, version, error messages.

Describe 'once --help'
  It 'prints usage and exits 0'
    When call "$ONCE_BIN" --help
    The status should be success
    The stdout should include 'once'
    The stdout should include 'build'
    The stdout should include 'run'
    The stdout should include 'exec'
    The stdout should include 'test'
    The stdout should include 'cache'
    The stdout should include 'auth'
    The stdout should include 'query'
    The stdout should include 'runtime'
    The stdout should include '--directory'
    The stdout should include '--list'
    The stdout should include 'toon'
  End
End

Describe 'once'
  It 'prints root help when no command is provided'
    When call "$ONCE_BIN"
    The status should equal 2
    The stderr should include 'Usage: once [OPTIONS] [COMMAND]'
    The stderr should include 'run'
    The stderr should include '--list'
  End
End

Describe 'once --list'
  It 'prints the whole command surface'
    When call "$ONCE_BIN" --list
    The status should be success
    The stdout should include 'once'
    The stdout should include 'build'
    The stdout should include 'run'
    The stdout should include 'exec'
    The stdout should include 'test'
    The stdout should include 'auth'
    The stdout should include 'query'
    The stdout should include 'runtime'
    The stdout should include '--list'
  End

  It 'prints a subtree when run under a namespace command'
    When call "$ONCE_BIN" cache --list
    The status should be success
    The stdout should include 'cache'
    The stdout should include 'blob'
  End

  It 'emits structured output for agents'
    When call "$ONCE_BIN" --format json --list
    The status should be success
    The stdout should include '"name":"once"'
    The stdout should include '"subcommands":'
    The stdout should include '"name":"run"'
    The stdout should include '"syntax":"--list"'
  End
End

Describe 'once --version'
  It 'prints the version'
    When call "$ONCE_BIN" --version
    The status should be success
    The stdout should include 'once'
  End
End

Describe 'once exec -e'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'rejects a malformed env entry without an equals sign'
    When call once exec -e MALFORMED -- /bin/sh -c 'true'
    The status should not equal 0
    The stderr should include 'KEY=VALUE'
  End

  It 'logs argument parsing failures to an internal session log'
    When call once exec -e MALFORMED -- /bin/sh -c 'true'
    The status should not equal 0
    The stderr should include 'KEY=VALUE'
    log_dir="$(once_log_dir)"
    log_file="$(find "$log_dir" -type f -name '*.log' | sort | head -n 1)"
    The path "$log_file" should be file
    The contents of file "$log_file" should include '"message":"argument parsing stopped"'
    The contents of file "$log_file" should include '"session_id"'
  End
End
