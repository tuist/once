#shellcheck shell=bash
# End-to-end specs for `fabrik deps sync`.

Describe 'fabrik deps sync'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'writes a declared Rust dependency graph'
    mkdir -p "$WORKSPACE/src" "$WORKSPACE/dep/src"
    cat > "$WORKSPACE/Cargo.toml" <<'EOF'
[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
dep = { path = "dep" }
EOF
    cat > "$WORKSPACE/src/lib.rs" <<'EOF'
pub fn app() -> &'static str { dep::dep() }
EOF
    cat > "$WORKSPACE/dep/Cargo.toml" <<'EOF'
[package]
name = "dep"
version = "0.1.0"
edition = "2021"
EOF
    cat > "$WORKSPACE/dep/src/lib.rs" <<'EOF'
pub fn dep() -> &'static str { "dep" }
EOF
    cargo generate-lockfile --manifest-path "$WORKSPACE/Cargo.toml" >/dev/null 2>&1
    cat > "$WORKSPACE/fabrik.toml" <<'EOF'
[[deps]]
name = "rust_deps"
ecosystem = "rust"
manifest = "Cargo.toml"
lockfile = "Cargo.lock"
output = "vendor/fabrik.rust.lock.json"
EOF

    When call "$FABRIK_BIN" --format json -C "$WORKSPACE" deps sync rust_deps
    The status should be success
    The stdout should include '"name":"rust_deps"'
    The stdout should include '"ecosystem":"rust"'
    The path "$WORKSPACE/vendor/fabrik.rust.lock.json" should be file
    The path "$WORKSPACE/vendor/rust_deps/fabrik.toml" should be file
    The contents of file "$WORKSPACE/vendor/fabrik.rust.lock.json" should include '"name": "dep"'
    The contents of file "$WORKSPACE/vendor/fabrik.rust.lock.json" should include '"kind": "path"'
  End

  It 'reuses cached Rust dependency resolution'
    mkdir -p "$WORKSPACE/src"
    cat > "$WORKSPACE/Cargo.toml" <<'EOF'
[package]
name = "app"
version = "0.1.0"
edition = "2021"
EOF
    cat > "$WORKSPACE/src/lib.rs" <<'EOF'
pub fn app() -> &'static str { "app" }
EOF
    cargo generate-lockfile --manifest-path "$WORKSPACE/Cargo.toml" >/dev/null 2>&1
    cat > "$WORKSPACE/fabrik.toml" <<'EOF'
[[deps]]
name = "rust_deps"
ecosystem = "rust"
manifest = "Cargo.toml"
lockfile = "Cargo.lock"
EOF

    "$FABRIK_BIN" -C "$WORKSPACE" deps sync rust_deps >/dev/null 2>&1
    When call "$FABRIK_BIN" -C "$WORKSPACE" deps sync rust_deps
    The status should be success
    The stderr should include 'resolution hit'
  End

  It 'warns when the deprecated vendor alias is used'
    mkdir -p "$WORKSPACE/src"
    cat > "$WORKSPACE/Cargo.toml" <<'EOF'
[package]
name = "app"
version = "0.1.0"
edition = "2021"
EOF
    cat > "$WORKSPACE/src/lib.rs" <<'EOF'
pub fn app() -> &'static str { "app" }
EOF
    cargo generate-lockfile --manifest-path "$WORKSPACE/Cargo.toml" >/dev/null 2>&1
    cat > "$WORKSPACE/fabrik.toml" <<'EOF'
[[deps]]
name = "rust_deps"
ecosystem = "rust"
manifest = "Cargo.toml"
lockfile = "Cargo.lock"
EOF

    When call "$FABRIK_BIN" -C "$WORKSPACE" vendor
    The status should be success
    The stderr should include '`fabrik vendor` is deprecated'
    The stderr should include 'fabrik deps sync'
  End

  It 'writes a declared Swift Package.resolved graph'
    cat > "$WORKSPACE/fabrik.toml" <<'EOF'
[[deps]]
name = "swift_deps"
ecosystem = "swift"
manifest = "Package.swift"
lockfile = "Package.resolved"
output = "vendor/fabrik.swift.lock.json"
EOF
    cat > "$WORKSPACE/Package.resolved" <<'EOF'
{
  "pins": [
    {
      "identity": "swift-log",
      "kind": "remoteSourceControl",
      "location": "https://github.com/apple/swift-log.git",
      "state": {
        "revision": "abc123",
        "version": "1.6.4"
      }
    }
  ],
  "version": 2
}
EOF

    When call "$FABRIK_BIN" --format json -C "$WORKSPACE" deps sync swift_deps
    The status should be success
    The stdout should include '"ecosystem":"swift"'
    The path "$WORKSPACE/vendor/fabrik.swift.lock.json" should be file
    The contents of file "$WORKSPACE/vendor/fabrik.swift.lock.json" should include '"name": "swift-log"'
    The contents of file "$WORKSPACE/vendor/fabrik.swift.lock.json" should include '"revision": "abc123"'
  End

  It 'writes a declared Go module graph'
    go_missing() { ! command -v go >/dev/null 2>&1; }
    Skip if 'go is not installed' go_missing

    mkdir -p "$WORKSPACE/app" "$WORKSPACE/lib"
    cat > "$WORKSPACE/app/go.mod" <<'EOF'
module example.com/app

go 1.22

require example.com/lib v0.0.0

replace example.com/lib => ../lib
EOF
    cat > "$WORKSPACE/lib/go.mod" <<'EOF'
module example.com/lib

go 1.22
EOF
    cat > "$WORKSPACE/fabrik.toml" <<'EOF'
[[deps]]
name = "go_deps"
ecosystem = "go"
manifest = "app/go.mod"
output = "vendor/fabrik.go.lock.json"
EOF

    When call "$FABRIK_BIN" --format json -C "$WORKSPACE" deps sync go_deps
    The status should be success
    The stdout should include '"ecosystem":"go"'
    The path "$WORKSPACE/vendor/fabrik.go.lock.json" should be file
    The contents of file "$WORKSPACE/vendor/fabrik.go.lock.json" should include '"name": "example.com/lib"'
    The contents of file "$WORKSPACE/vendor/fabrik.go.lock.json" should include '"replace"'
  End

  It 'reuses cached Go dependency resolution'
    go_missing() { ! command -v go >/dev/null 2>&1; }
    Skip if 'go is not installed' go_missing
    go_missing && return 0

    mkdir -p "$WORKSPACE/app" "$WORKSPACE/lib"
    cat > "$WORKSPACE/app/go.mod" <<'EOF'
module example.com/app

go 1.22

require example.com/lib v0.0.0

replace example.com/lib => ../lib
EOF
    cat > "$WORKSPACE/lib/go.mod" <<'EOF'
module example.com/lib

go 1.22
EOF
    cat > "$WORKSPACE/fabrik.toml" <<'EOF'
[[deps]]
name = "go_deps"
ecosystem = "go"
manifest = "app/go.mod"
EOF

    "$FABRIK_BIN" -C "$WORKSPACE" deps sync go_deps >/dev/null 2>&1
    When call "$FABRIK_BIN" -C "$WORKSPACE" deps sync go_deps
    The status should be success
    The stderr should include 'resolution hit'
  End

  It 'writes a declared Elixir mix.lock graph'
    cat > "$WORKSPACE/fabrik.toml" <<'EOF'
[[deps]]
name = "elixir_deps"
ecosystem = "elixir"
manifest = "mix.exs"
lockfile = "mix.lock"
output = "vendor/fabrik.elixir.lock.json"
EOF
    cat > "$WORKSPACE/mix.lock" <<'EOF'
%{
  "decimal": {:hex, :decimal, "2.1.1", "inner", [:mix], [], "hexpm", "outer"},
  "jason": {:hex, :jason, "1.4.1", "inner2", [:mix], [{:decimal, "~> 2.0", [hex: :decimal, repo: "hexpm", optional: true]}], "hexpm", "outer2"}
}
EOF

    When call "$FABRIK_BIN" --format json -C "$WORKSPACE" deps sync elixir_deps
    The status should be success
    The stdout should include '"ecosystem":"elixir"'
    The path "$WORKSPACE/vendor/fabrik.elixir.lock.json" should be file
    The contents of file "$WORKSPACE/vendor/fabrik.elixir.lock.json" should include '"name": "jason"'
    The contents of file "$WORKSPACE/vendor/fabrik.elixir.lock.json" should include '"name": "decimal"'
  End
End
