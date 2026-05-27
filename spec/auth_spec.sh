#shellcheck shell=bash

Describe 'fabrik auth'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'prints auth help and lists login and logout'
    When call "$FABRIK_BIN" auth --help
    The status should be success
    The stdout should include 'login'
    The stdout should include 'logout'
  End

  It 'rejects login when the workspace provider is local'
    cat > "$WORKSPACE/fabrik.toml" <<'EOF'
[cache_provider]
kind = "local"
EOF

    When call fabrik auth login --provider workspace
    The status should be failure
    The stderr should include 'does not support authentication'
  End

  It 'logs out of the built-in Tuist provider without a stored session'
    When call fabrik auth logout --provider tuist
    The status should be success
    The stdout should include 'no stored session for'
    The stdout should include 'tuist'
  End
End
