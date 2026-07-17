#shellcheck shell=bash
# End-to-end specs for the agent-facing graph discovery and editing
# CLI surface: `once query target-kinds`, `once query example`,
# `once query target`, `once query validate-target`, and
# `once edit apply`. These commands mirror the MCP tool surface; the
# MCP tests cover the protocol layer, these specs pin the CLI contract
# (exit codes, --format handling, stdin and --file input).

seed_custom_module_workspace() {
  mkdir -p "$WORKSPACE/modules"
  cat > "$WORKSPACE/once.toml" <<'EOF'
[modules]
paths = ["modules/*.star"]
EOF
  cat > "$WORKSPACE/modules/demo.star" <<'EOF'
demo_kind = target_kind(
    docs = "Demo kind",
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

Describe 'once query target-kinds'
  It 'lists every target kind with its example slugs in human mode'
    When call "$ONCE_BIN" query target-kinds
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
    When call "$ONCE_BIN" --format json query target-kinds
    The status should be success
    The stdout should include '"kind":"apple_library"'
    The stdout should include '"slug":"apple-library-minimal"'
    The stdout should include '"use_when"'
  End

  It 'filters discovery by ecosystem intent'
    When call "$ONCE_BIN" --format json query target-kinds --query elixir
    The status should be success
    The stdout should include '"kind":"elixir_library"'
    The stdout should include '"kind":"elixir_test"'
    The stdout should not include '"kind":"apple_application"'
  End

  It 'prioritizes a named family over generic intent words'
    When call "$ONCE_BIN" --format json query target-kinds --query 'typed Rust library executable test'
    The status should be success
    The stdout should include '"kind":"rust_library"'
    The stdout should include '"kind":"rust_binary"'
    The stdout should include '"kind":"rust_test"'
    The stdout should not include '"kind":"apple_application"'
    The stdout should not include '"kind":"zig_test"'
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

Describe 'workspace custom module CLI surface'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'includes custom target kinds in query target-kinds'
    seed_custom_module_workspace
    When call "$ONCE_BIN" -C "$WORKSPACE" --format json query target-kinds
    The status should be success
    The stdout should include '"kind":"demo_kind"'
    The stdout should include '"docs":"Demo kind"'
  End

  It 'returns custom target kind schemas from query schema'
    seed_custom_module_workspace
    When call "$ONCE_BIN" -C "$WORKSPACE" --format json query schema demo_kind
    The status should be success
    The stdout should include '"kind":"demo_kind"'
    The stdout should include '"name":"message"'
    The stdout should include '"required":true'
  End

  It 'validates drafts against custom target kind schemas'
    seed_custom_module_workspace
    cat > "$WORKSPACE/draft.json" <<'EOF'
{"target":{"name":"Hello","kind":"demo_kind","attrs":{"message":"hello"}}}
EOF
    When call "$ONCE_BIN" -C "$WORKSPACE" --format json query validate-target --file "$WORKSPACE/draft.json"
    The status should be success
    The stdout should include '"valid":true'
  End

  It 'applies edits against custom target kind schemas'
    seed_custom_module_workspace
    cat > "$WORKSPACE/edit.json" <<'EOF'
{"package":"apps/Hello","operations":[{"op":"create","target":{"name":"Hello","kind":"demo_kind","attrs":{"message":"hello"}}}]}
EOF
    When call "$ONCE_BIN" -C "$WORKSPACE" --format json edit apply --file "$WORKSPACE/edit.json"
    The status should be success
    The stdout should include '"applied":true'
    The path "$WORKSPACE/apps/Hello/once.toml" should be file
    The contents of file "$WORKSPACE/apps/Hello/once.toml" should include 'kind = "demo_kind"'
  End
End

Describe 'once query expression'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  seed_query_expression_workspace() {
    mkdir -p "$WORKSPACE/apps/Hello"
    cat > "$WORKSPACE/apps/Hello/once.toml" <<'EOF'
[[target]]
name = "Core"
kind = "apple_library"

[target.attrs]
platform = "ios"

[[target]]
name = "DesignSystem"
kind = "apple_framework"
deps = ["./Core"]

[target.attrs]
platform = "ios"
bundle_id = "dev.once.DesignSystem"

[[target]]
name = "App"
kind = "apple_application"
deps = ["./DesignSystem"]

[target.attrs]
platform = "ios"
bundle_id = "dev.once.App"

[[target]]
name = "AppTests"
kind = "apple_test_bundle"
deps = ["./Core"]

[target.attrs]
platform = "ios"
EOF
  }

  It 'evaluates a transitive dependency query'
    seed_query_expression_workspace
    When call once query 'MATCH (app:Target {id: "apps/Hello/App"})-[:DEPENDS_ON*]->(dep:Target) RETURN dep.id, dep.kind'
    The status should be success
    The stdout should include 'dep.id | dep.kind'
    The stdout should include 'apps/Hello/DesignSystem | apple_framework'
    The stdout should include 'apps/Hello/Core | apple_library'
  End

  It 'emits JSON rows for capability queries'
    seed_query_expression_workspace
    When call once --format json query 'MATCH (t:Target)-[:EXPOSES]->(c:Capability {name: "test"}) RETURN t.id AS target'
    The status should be success
    The stdout should include '"columns":["target"]'
    The stdout should include '"apps/Hello/AppTests"'
  End

  It 'emits Toon rows for capability queries'
    seed_query_expression_workspace
    When call once --format toon query 'MATCH (t:Target)-[:EXPOSES]->(c:Capability {name: "test"}) RETURN t.id AS target'
    The status should be success
    The stdout should include 'columns[1]:target'
    The stdout should include 'apps/Hello/AppTests'
  End

  It 'treats bare supported label words as aliases'
    seed_query_expression_workspace
    When call once query 'MATCH (Target) RETURN Target.id'
    The status should be success
    The stdout should include 'Target.id'
    The stdout should include 'apps/Hello/App'
  End

  It 'rejects invalid Cypher syntax'
    seed_query_expression_workspace
    When call once query 'MATCH (t:Target RETURN t.id'
    The status should equal 2
    The stdout should equal ''
    The stderr should include 'not valid Cypher syntax'
  End

  It 'rejects unsupported graph labels'
    seed_query_expression_workspace
    When call once query 'MATCH (t:Unknown) RETURN t.id'
    The status should equal 2
    The stdout should equal ''
    The stderr should include 'unsupported graph label'
  End

  It 'rejects unsupported graph relationships'
    seed_query_expression_workspace
    When call once query 'MATCH (t:Target)-[:OWNS]->(d:Target) RETURN d.id'
    The status should equal 2
    The stdout should equal ''
    The stderr should include 'unsupported relationship'
  End

  It 'rejects mutating Cypher clauses'
    seed_query_expression_workspace
    When call once query 'MATCH (t:Target) DELETE t RETURN t.id'
    The status should equal 2
    The stdout should equal ''
    The stderr should include 'read-only'
    The stderr should include 'DELETE'
  End

  It 'rejects optional match clauses'
    seed_query_expression_workspace
    When call once query 'OPTIONAL MATCH (t:Target) RETURN t.id'
    The status should equal 2
    The stdout should equal ''
    The stderr should include 'read-only'
    The stderr should include 'OPTIONAL'
  End

  It 'rejects drop statements'
    seed_query_expression_workspace
    When call once query 'DROP INDEX target_name'
    The status should equal 2
    The stdout should equal ''
    The stderr should include 'read-only'
    The stderr should include 'DROP'
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
    The stdout should include '"schema":"once.error.v1"'
    The stdout should include 'validate-target input'
    The stderr should include 'session failed'
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
    The stdout should include '"schema":"once.error.v1"'
    The stdout should include 'apply input'
    The stderr should include 'session failed'
  End
End
