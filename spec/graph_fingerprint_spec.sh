#shellcheck shell=bash
# End-to-end specs for the whole-graph content fingerprint.

Describe 'once query graph-fingerprint'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  setup_graph_workspace() {
    cat > "$WORKSPACE/once.toml" <<'EOF'
[workspace]
include = ["once.toml", "pkg/once.toml"]
EOF
    mkdir -p "$WORKSPACE/pkg"
    cat > "$WORKSPACE/pkg/once.toml" <<'EOF'
[[target]]
name = "lib"
kind = "library"
srcs = ["lib.rs"]
EOF
    printf '%s\n' 'fn lib() {}' > "$WORKSPACE/pkg/lib.rs"
    cat > "$WORKSPACE/mise.toml" <<'EOF'
[tools]
rust = "1.96.0"
EOF
  }

  It 'returns the schema and categorized components'
    setup_graph_workspace
    When call once query graph-fingerprint --format json
    The status should be success
    The stdout should include '"once.graph_fingerprint.v1"'
    The stdout should include '"category":"target"'
    The stdout should include '"category":"source"'
    The stdout should include '"category":"toolchain"'
    The stdout should include '"category":"manifest"'
  End

  It 'produces a stable digest across runs'
    setup_graph_workspace
    once query graph-fingerprint --format json | jq -r '.digest' > "$WORKSPACE/one"
    once query graph-fingerprint --format json | jq -r '.digest' > "$WORKSPACE/two"
    one="$(cat "$WORKSPACE/one")"
    two="$(cat "$WORKSPACE/two")"
    The value "$one" should equal "$two"
  End

  It 'changes the digest when a tracked source changes'
    setup_graph_workspace
    once query graph-fingerprint --format json | jq -r '.digest' > "$WORKSPACE/before"
    printf '%s\n' 'fn changed() {}' > "$WORKSPACE/pkg/lib.rs"
    once query graph-fingerprint --format json | jq -r '.digest' > "$WORKSPACE/after"
    before="$(cat "$WORKSPACE/before")"
    after="$(cat "$WORKSPACE/after")"
    The value "$before" should not equal "$after"
  End

  It 'changes the digest when the Mise toolchain declaration changes'
    cat > "$WORKSPACE/once.toml" <<'EOF'
[workspace]
include = ["once.toml"]
EOF
    cat > "$WORKSPACE/mise.toml" <<'EOF'
[tools]
rust = "1.96.0"
EOF
    once query graph-fingerprint --format json | jq -r '.digest' > "$WORKSPACE/before"
    cat > "$WORKSPACE/mise.toml" <<'EOF'
[tools]
rust = "1.97.0"
EOF
    once query graph-fingerprint --format json | jq -r '.digest' > "$WORKSPACE/after"
    before="$(cat "$WORKSPACE/before")"
    after="$(cat "$WORKSPACE/after")"
    The value "$before" should not equal "$after"
  End

  It 'excludes source contents from a structure-only digest'
    setup_graph_workspace
    once query graph-fingerprint --no-sources --format json | jq -r '.digest' > "$WORKSPACE/before"
    printf '%s\n' 'fn changed() {}' > "$WORKSPACE/pkg/lib.rs"
    once query graph-fingerprint --no-sources --format json | jq -r '.digest' > "$WORKSPACE/after"
    before="$(cat "$WORKSPACE/before")"
    after="$(cat "$WORKSPACE/after")"
    The value "$before" should equal "$after"
  End
End
