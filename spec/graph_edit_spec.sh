#shellcheck shell=bash
# End-to-end specs for the agent-facing graph discovery and editing
# CLI surface: `once query rules`, `once query example`,
# `once query target`, `once query validate-target`, and
# `once edit apply`. These commands mirror the MCP tool surface; the
# MCP tests cover the protocol layer, these specs pin the CLI contract
# (exit codes, --format handling, stdin and --file input).

seed_custom_rule_workspace() {
  mkdir -p "$WORKSPACE/rules"
  cat > "$WORKSPACE/once.toml" <<'EOF'
[rules]
paths = ["rules/*.star"]
EOF
  cat > "$WORKSPACE/rules/demo.star" <<'EOF'
demo_rule = rule(
    docs = "Demo rule",
    attrs = [
        attr("message", "string", required = True, docs = "Message to emit"),
    ],
    deps = [],
    providers = ["demo_provider"],
    capabilities = [
        capability("build", ["default"]),
    ],
)
EOF
}

Describe 'once query rules'
  It 'lists every rule kind with its example slugs in human mode'
    When call "$ONCE_BIN" query rules
    The status should be success
    The stdout should include 'apple_library'
    The stdout should include 'apple_application'
    The stdout should include 'rust_library'
    The stdout should include 'rust_crate'
    The stdout should include 'apple-library-minimal'
    The stdout should include 'apple-application-minimal'
    The stdout should include 'rust-library-minimal'
  End

  It 'emits structured JSON under --format json'
    When call "$ONCE_BIN" --format json query rules
    The status should be success
    The stdout should include '"kind":"apple_library"'
    The stdout should include '"slug":"apple-library-minimal"'
    The stdout should include '"use_when"'
  End
End

Describe 'once query example'
  It 'materializes a starter example as JSON'
    When call "$ONCE_BIN" --format json query example apple_library apple-library-minimal
    The status should be success
    The stdout should include '"slug":"apple-library-minimal"'
    The stdout should include '"path":"apps/Hello/once.toml"'
    The stdout should include '"contents":"[[target]]'
  End

  It 'materializes a starter example through MCP'
    mcp_request='{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"once_query_example","arguments":{"kind":"apple_library","slug":"apple-library-minimal"}}}'

    When call /bin/sh -c 'printf "%s\n" "$1" | "$2" -C "$3" mcp' sh "$mcp_request" "$ONCE_BIN" "$REPO_ROOT"
    The status should be success
    The stdout should include 'apple-library-minimal'
    The stdout should include 'apps/Hello/once.toml'
    The stdout should include 'Hello.swift'
  End
End

Describe 'workspace custom rule CLI surface'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'includes custom rules in query rules'
    seed_custom_rule_workspace
    When call "$ONCE_BIN" -C "$WORKSPACE" --format json query rules
    The status should be success
    The stdout should include '"kind":"demo_rule"'
    The stdout should include '"docs":"Demo rule"'
  End

  It 'returns custom schemas from query schema'
    seed_custom_rule_workspace
    When call "$ONCE_BIN" -C "$WORKSPACE" --format json query schema demo_rule
    The status should be success
    The stdout should include '"kind":"demo_rule"'
    The stdout should include '"name":"message"'
    The stdout should include '"required":true'
  End

  It 'validates drafts against custom schemas'
    seed_custom_rule_workspace
    cat > "$WORKSPACE/draft.json" <<'EOF'
{"target":{"name":"Hello","kind":"demo_rule","attrs":{"message":"hello"}}}
EOF
    When call "$ONCE_BIN" -C "$WORKSPACE" --format json query validate-target --file "$WORKSPACE/draft.json"
    The status should be success
    The stdout should include '"valid":true'
  End

  It 'applies edits against custom schemas'
    seed_custom_rule_workspace
    cat > "$WORKSPACE/edit.json" <<'EOF'
{"package":"apps/Hello","operations":[{"op":"create","target":{"name":"Hello","kind":"demo_rule","attrs":{"message":"hello"}}}]}
EOF
    When call "$ONCE_BIN" -C "$WORKSPACE" --format json edit apply --file "$WORKSPACE/edit.json"
    The status should be success
    The stdout should include '"applied":true'
    The path "$WORKSPACE/apps/Hello/once.toml" should be file
    The contents of file "$WORKSPACE/apps/Hello/once.toml" should include 'kind = "demo_rule"'
  End
End

Describe 'once query target'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  seed_apple_library_target() {
    mkdir -p "$WORKSPACE/apps/Hello"
    cat > "$WORKSPACE/apps/Hello/once.toml" <<'EOF'
[[target]]
name = "Hello"
kind = "apple_library"
srcs = ["Sources/**/*.swift"]

[target.attrs]
platform = "ios"
minimum_os = "17.0"
EOF
  }

  It 'resolves a target by id and prints its kind and attrs'
    seed_apple_library_target
    When call once query target apps/Hello/Hello
    The status should be success
    The stdout should include 'apps/Hello/Hello (apple_library)'
    The stdout should include 'platform'
    The stdout should include 'ios'
  End

  It 'emits a structured record under --format json'
    seed_apple_library_target
    When call once --format json query target apps/Hello/Hello
    The status should be success
    The stdout should include '"id":"apps/Hello/Hello"'
    The stdout should include '"kind":"apple_library"'
    The stdout should include '"platform":"ios"'
  End

  It 'fails when the target id does not resolve'
    seed_apple_library_target
    When call once query target apps/Hello/Missing
    The status should not equal 0
    The stderr should include 'no target matches'
  End
End

Describe 'once query validate-target'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'returns valid for a minimal apple_library draft via stdin'
    Data
      #|{"target":{"name":"Hello","kind":"apple_library","attrs":{"platform":"ios"}}}
    End
    When call "$ONCE_BIN" --format json query validate-target
    The status should be success
    The stdout should include '"valid":true'
  End

  It 'returns structured diagnostics for a missing required attr'
    Data
      #|{"target":{"name":"Hello","kind":"apple_library"}}
    End
    When call "$ONCE_BIN" --format json query validate-target
    The status should be success
    The stdout should include '"valid":false'
    The stdout should include '"code":"missing_required_attr"'
    The stdout should include '"attribute":"platform"'
    The stdout should include '"target":"Hello"'
  End

  It 'accepts the JSON document from --file instead of stdin'
    cat > "$WORKSPACE/draft.json" <<'EOF'
{"target":{"name":"Hello","kind":"apple_library","attrs":{"platform":"ios"}}}
EOF
    When call "$ONCE_BIN" --format json query validate-target --file "$WORKSPACE/draft.json"
    The status should be success
    The stdout should include '"valid":true'
  End

  It 'fails with context when the JSON does not match the expected shape'
    Data
      #|{"wrong":"shape"}
    End
    When call "$ONCE_BIN" --format json query validate-target
    The status should not equal 0
    The stderr should include 'validate-target input'
  End
End

Describe 'once edit apply'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'creates the manifest from nothing using a create operation via stdin'
    Data
      #|{"package":"apps/Hello","operations":[{"op":"create","target":{"name":"Hello","kind":"apple_library","attrs":{"platform":"ios"}}}]}
    End
    When call "$ONCE_BIN" -C "$WORKSPACE" --format json edit apply
    The status should be success
    The stdout should include '"applied":true'
    The path "$WORKSPACE/apps/Hello/once.toml" should be file
  End

  It 'reads the operation batch from --file instead of stdin'
    cat > "$WORKSPACE/edit.json" <<'EOF'
{"package":"apps/Hello","operations":[{"op":"create","target":{"name":"Hello","kind":"apple_library","attrs":{"platform":"ios"}}}]}
EOF
    When call "$ONCE_BIN" -C "$WORKSPACE" --format json edit apply --file "$WORKSPACE/edit.json"
    The status should be success
    The stdout should include '"applied":true'
    The path "$WORKSPACE/apps/Hello/once.toml" should be file
  End

  It 'returns structured diagnostics and leaves the file untouched on failure'
    mkdir -p "$WORKSPACE/apps/Hello"
    cat > "$WORKSPACE/apps/Hello/once.toml" <<'EOF'
[[target]]
name = "Hello"
kind = "apple_library"
EOF
    original_sha="$(shasum "$WORKSPACE/apps/Hello/once.toml" | awk '{print $1}')"
    Data
      #|{"package":"apps/Hello","operations":[{"op":"delete","target_name":"Missing"}]}
    End
    When call "$ONCE_BIN" -C "$WORKSPACE" --format json edit apply
    The status should be success
    The stdout should include '"applied":false'
    The stdout should include '"code":"target_not_found"'
    new_sha="$(shasum "$WORKSPACE/apps/Hello/once.toml" | awk '{print $1}')"
    The variable new_sha should equal "$original_sha"
  End

  It 'rejects input that does not match the expected JSON shape'
    Data
      #|not a json object
    End
    When call "$ONCE_BIN" -C "$WORKSPACE" --format json edit apply
    The status should not equal 0
    The stderr should include 'apply input'
  End
End
