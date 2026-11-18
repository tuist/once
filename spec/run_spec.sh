#shellcheck shell=bash
# End-to-end specs for `struo run`.

Describe 'struo run'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'errors when the target id does not match any target'
    touch "$WORKSPACE/struo.toml"
    When call struo run nope
    The status should not equal 0
    The stderr should include 'no target matches'
  End

  It 'rejects script declarations in struo.toml'
    cat > "$WORKSPACE/struo.toml" <<'EOF'
[[script]]
name = "hello"
argv = ["/bin/sh", "-c", "printf hello"]
EOF
    When call struo run hello
    The status should not equal 0
    The stderr should include 'unknown field `script`'
  End
End
