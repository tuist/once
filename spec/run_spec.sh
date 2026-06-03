#shellcheck shell=bash
# End-to-end specs for `once run`.

Describe 'once run'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'runs a manifest-backed script target declared in once.toml'
    mkdir -p "$WORKSPACE/scripts"
    cat > "$WORKSPACE/scripts/input.txt" <<'EOF'
hello inline script
EOF
    cat > "$WORKSPACE/scripts/once.toml" <<'EOF'
[[target]]
name = "print"
rule = "script"

[target.script]
argv = ["/bin/sh", "-c", "cat input.txt"]
input = ["input.txt"]
cwd = "scripts"
EOF
    When call once run scripts/print
    The status should be success
    The stdout should equal 'hello inline script'
    The stderr should include 'cache miss'
  End

  It 'runs a script target and restores its declared output'
    mkdir -p "$WORKSPACE/scripts/scripts"
    cat > "$WORKSPACE/scripts/scripts/build.sh" <<'EOF'
#!/bin/sh
# ONCE input "../input.txt"
# ONCE output "../out.txt"
# ONCE cwd ".."
printf '%s' "$(cat input.txt)" > out.txt
printf '%s' "$(cat out.txt)"
EOF
    chmod +x "$WORKSPACE/scripts/scripts/build.sh"
    cat > "$WORKSPACE/scripts/input.txt" <<'EOF'
hello script
EOF
    cat > "$WORKSPACE/scripts/once.toml" <<'EOF'
[[target]]
name = "build"
rule = "script"

[target.script]
path = "scripts/build.sh"
EOF
    When call once run scripts/build
    The status should be success
    The stdout should equal 'hello script'
    The stderr should include 'cache miss'
    The contents of file "$WORKSPACE/scripts/out.txt" should equal 'hello script'
  End

  It 'reuses the cache for a script target'
    mkdir -p "$WORKSPACE/scripts/scripts"
    cat > "$WORKSPACE/scripts/scripts/build.sh" <<'EOF'
#!/bin/sh
# ONCE input "../input.txt"
# ONCE output "../out.txt"
# ONCE cwd ".."
printf '%s' "$(cat input.txt)" > out.txt
printf '%s' "$(cat out.txt)"
EOF
    chmod +x "$WORKSPACE/scripts/scripts/build.sh"
    cat > "$WORKSPACE/scripts/input.txt" <<'EOF'
cached script
EOF
    cat > "$WORKSPACE/scripts/once.toml" <<'EOF'
[[target]]
name = "build"
rule = "script"

[target.script]
path = "scripts/build.sh"
EOF
    once run scripts/build >/dev/null 2>&1
    When call once run scripts/build
    The status should be success
    The stdout should equal 'cached script'
    The stderr should include 'cache hit'
  End

  It 'runs a script target through the microsandbox compute provider'
    Skip if 'microsandbox specs are opt-in' microsandbox_specs_disabled
    mkdir -p "$WORKSPACE/scripts"
    cat > "$WORKSPACE/scripts/once.toml" <<'EOF'
[[target]]
name = "remote"
rule = "script"

[target.script]
argv = ["/bin/sh", "-c", "printf remote-output"]
remote = "microsandbox"
EOF
    When call "$ONCE_BIN" -C "$WORKSPACE" run scripts/remote
    The status should be success
    The stdout should equal 'remote-output'
    The stderr should include 'cache miss'
  End

  It 'runs a runtime script target and emits its runtime interfaces'
    mkdir -p "$WORKSPACE/tasks"
    cat > "$WORKSPACE/tasks/once.toml" <<'EOF'
[[target]]
name = "launch"
rule = "script"

[target.script]
argv = ["/bin/sh", "-c", "printf launched"]

[target.runtime]
kind = "web_server"
capabilities = ["logs", "http"]

[[target.runtime.interface]]
name = "logs"
kind = "stream"
argv = ["tail", "-f", "server.log"]
description = "Stream server logs"
EOF
    When call "$ONCE_BIN" --format json -C "$WORKSPACE" run tasks/launch
    The status should be success
    The stdout should include '"kind":"runtime_script"'
    The stdout should include '"runtime":{"kind":"web_server"'
    The stdout should include '"capabilities":["logs","http"]'
    The stdout should include '"name":"logs"'
  End

  It 'errors when the target id does not match any target'
    cat > "$WORKSPACE/once.toml" <<'EOF'
[[script]]
name = "hello"
argv = ["/bin/sh", "-c", "printf hello"]
EOF
    When call once run nope
    The status should not equal 0
    The stderr should include 'no target matches'
  End

  It 'errors on an unsupported target kind'
    cat > "$WORKSPACE/once.toml" <<'EOF'
[[target]]
name = "lib"
rule = "rust.library"
EOF
    When call once run lib
    The status should not equal 0
    The stderr should include 'Once only supports `script`'
  End

  It 'emits a JSON outcome record on stdout under --format json'
    cat > "$WORKSPACE/once.toml" <<'EOF'
[[script]]
name = "hello"
argv = ["/bin/sh", "-c", "printf hello"]
EOF
    When call "$ONCE_BIN" --format json -C "$WORKSPACE" run hello
    The status should be success
    The stdout should include '"target":"hello"'
    The stdout should include '"cache":"miss"'
    The stdout should include '"exit_code":0'
    The stdout should include '"action_digest":'
  End
End
