#shellcheck shell=bash
# Surface checks for the `fabrik cargo` subcommand. End-to-end caching
# behavior is covered by Rust unit tests in fabrik-rust where the
# assertion can inspect the action digest directly.

Describe 'fabrik cargo --help'
  It 'documents the subcommand and the cache-failures flag'
    When call "$FABRIK_BIN" cargo --help
    The status should be success
    The stdout should include 'cargo'
    The stdout should include '--cache-failures'
  End
End

Describe 'fabrik cargo without args'
  It 'errors instead of running cargo with no arguments'
    When call "$FABRIK_BIN" cargo
    The status should not equal 0
    The stderr should include 'required'
  End
End

Describe 'fabrik cargo against a non-rust workspace'
  setup() {
    fresh="$(mktemp -d -t fabrik-cargo-spec.XXXXXX)"
  }
  cleanup() {
    rm -rf "$fresh"
  }
  BeforeEach 'setup'
  AfterEach 'cleanup'

  It 'errors clearly when the workspace has no Cargo.toml'
    When call "$FABRIK_BIN" -C "$fresh" cargo -- check
    The status should not equal 0
    The stderr should include 'not a Rust workspace'
  End
End
