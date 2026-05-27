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

  It 'prints login help with provider and no-browser options'
    When call "$FABRIK_BIN" auth login --help
    The status should be success
    The stdout should include '--provider'
    The stdout should include '--no-browser'
  End

  It 'prints logout help with the provider option'
    When call "$FABRIK_BIN" auth logout --help
    The status should be success
    The stdout should include '--provider'
  End

  It 'lists the auth command surface'
    When call "$FABRIK_BIN" auth --list
    The status should be success
    The stdout should include 'auth'
    The stdout should include 'login'
    The stdout should include 'logout'
    The stdout should include '--provider'
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

  It 'errors when an auth provider is unknown'
    When call fabrik auth logout --provider missing
    The status should be failure
    The stderr should include 'auth provider `missing` was not found'
  End

  It 'logs out of the built-in Tuist provider without a stored session'
    When call fabrik auth logout --provider tuist
    The status should be success
    The stdout should include 'no stored session for'
    The stdout should include 'tuist'
  End

  It 'resolves a named user infrastructure for auth logout'
    mkdir -p "$XDG_CONFIG_HOME/fabrik"
    cat > "$XDG_CONFIG_HOME/fabrik/config.toml" <<'EOF'
[infrastructures.acme]
kind = "tuist"
url = "https://tuist.dev"
account = "acme"
EOF

    When call fabrik auth logout --provider acme
    The status should be success
    The stdout should include 'no stored session for'
    The stdout should include 'acme'
  End

  It 'resolves workspace auth through user infrastructure binding'
    mkdir -p "$XDG_CONFIG_HOME/fabrik"
    cat > "$WORKSPACE/fabrik.toml" <<'EOF'
[infrastructure.cache]
name = "acme"
account = "acme"
EOF
    cat > "$XDG_CONFIG_HOME/fabrik/config.toml" <<'EOF'
[infrastructures.acme]
kind = "tuist"
url = "https://tuist.dev"
EOF

    When call fabrik auth logout --provider workspace
    The status should be success
    The stdout should include 'no stored session for'
    The stdout should include 'acme'
  End
End
