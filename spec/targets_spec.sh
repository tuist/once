#shellcheck shell=bash
# End-to-end specs for `fabrik targets`.

Describe 'fabrik targets'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'lists every target across packages, sorted with the root package first'
    cat > "$WORKSPACE/fabrik.toml" <<'EOF'
[[rust.binary]]
name = "top"
srcs = ["main.rs"]
EOF
    mkdir -p "$WORKSPACE/crates/a" "$WORKSPACE/crates/b"
    cat > "$WORKSPACE/crates/a/fabrik.toml" <<'EOF'
[[rust.library]]
name = "a"
srcs = ["lib.rs"]
EOF
    cat > "$WORKSPACE/crates/b/fabrik.toml" <<'EOF'
[[rust.binary]]
name = "b"
srcs = ["main.rs"]

[[rust.test]]
name = "b_test"
srcs = ["tests/b.rs"]
EOF
    When call fabrik targets
    The status should be success
    The line 1 of stdout should equal 'rust_binary top'
    The line 2 of stdout should equal 'rust_library crates/a/a'
    The line 3 of stdout should equal 'rust_binary crates/b/b'
    The line 4 of stdout should equal 'rust_test crates/b/b_test'
  End

  It 'expands glob patterns against the package directory'
    mkdir -p "$WORKSPACE/pkg/src"
    : > "$WORKSPACE/pkg/src/main.rs"
    : > "$WORKSPACE/pkg/src/lib.rs"
    cat > "$WORKSPACE/pkg/fabrik.toml" <<'EOF'
[[rust.binary]]
name = "pkg"
src_globs = ["src/*.rs"]
EOF
    When call fabrik targets
    The status should be success
    The stdout should equal 'rust_binary pkg/pkg'
  End

  It 'lists targets declared in fabrik.toml'
    mkdir -p "$WORKSPACE/pkg/src"
    : > "$WORKSPACE/pkg/src/lib.rs"
    cat > "$WORKSPACE/pkg/fabrik.toml" <<'EOF'
[[rust.library]]
name = "pkg"
srcs = ["src/lib.rs"]
EOF
    When call fabrik targets
    The status should be success
    The stdout should equal 'rust_library pkg/pkg'
  End

  It 'skips hidden directories like .fabrik and .git'
    cat > "$WORKSPACE/fabrik.toml" <<'EOF'
[[rust.binary]]
name = "visible"
srcs = ["main.rs"]
EOF
    mkdir -p "$WORKSPACE/.fabrik/sub"
    cat > "$WORKSPACE/.fabrik/sub/fabrik.toml" <<'EOF'
[[rust.binary]]
name = "hidden"
srcs = ["x.rs"]
EOF
    When call fabrik targets
    The status should be success
    The stdout should equal 'rust_binary visible'
  End

  It 'prints nothing when the project has no fabrik.toml files'
    When call fabrik targets
    The status should be success
    The stdout should equal ''
  End

  It 'reports a parse error from the offending file and exits non-zero'
    printf '[[rust.binary]\n' > "$WORKSPACE/fabrik.toml"
    When call fabrik targets
    The status should not equal 0
    The stderr should include 'parse error'
  End

  It 'reports an evaluation error when the schema is violated'
    cat > "$WORKSPACE/fabrik.toml" <<'EOF'
[[rust.binary]]
name = "bad/name"
srcs = ["a.rs"]
EOF
    When call fabrik targets
    The status should not equal 0
    The stderr should include 'evaluation error'
  End

  It 'emits one JSON record per target under --format json (NDJSON)'
    cat > "$WORKSPACE/fabrik.toml" <<'EOF'
[[rust.binary]]
name = "top"
srcs = ["main.rs"]
deps = [{ cargo = "serde" }]

[[rust.library]]
name = "lib"
srcs = ["lib.rs"]
EOF
    When call "$FABRIK_BIN" --format json -C "$WORKSPACE" targets
    The status should be success
    The stdout should include '"id":"top"'
    The stdout should include '"kind":"rust_binary"'
    The stdout should include '"external_deps":[{"graph":"cargo","spec":"serde"}]'
    The stdout should include '"id":"lib"'
    The stdout should include '"kind":"rust_library"'
  End

  It 'emits a single TOON document under --format toon'
    cat > "$WORKSPACE/fabrik.toml" <<'EOF'
[[rust.binary]]
name = "top"
srcs = ["main.rs"]

[[rust.library]]
name = "lib"
srcs = ["lib.rs"]
EOF
    When call "$FABRIK_BIN" --format toon -C "$WORKSPACE" targets
    The status should be success
    The stdout should include 'targets:'
    The stdout should include 'top:'
    The stdout should include 'lib:'
    The stdout should include 'kind: rust_binary'
    The stdout should include 'kind: rust_library'
  End
End
