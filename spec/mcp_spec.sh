#shellcheck shell=bash
# End-to-end specs for the MCP stdio protocol.

Describe 'once mcp'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'advertises and calls the evidence query tool over stdio'
    once exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'printf mcp-evidence' >/dev/null 2>&1
    cat > "$WORKSPACE/mcp.jsonl" <<'EOF'
{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"once_query_evidence","arguments":{}}}
EOF

    When call /bin/sh -c 'cat "$1" | "$2" -C "$3" mcp' sh "$WORKSPACE/mcp.jsonl" "$ONCE_BIN" "$WORKSPACE"
    The status should be success
    The stdout should include '"name":"once_query_evidence"'
    The stdout should include 'once.evidence.v1'
    The stdout should include 'action_result'
    The stdout should include 'passed'
  End
End
