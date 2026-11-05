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

  It 'validates and executes an annotated script through the cache'
    cat > "$WORKSPACE/build.sh" <<'SH'
#!/bin/sh
# once input "input.txt"
# once output "output.txt"
cp input.txt output.txt
SH
    printf '%s\n' 'hello from an annotated script' > "$WORKSPACE/input.txt"
    cat > "$WORKSPACE/mcp.jsonl" <<'EOF'
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"once_validate_script","arguments":{"path":"build.sh"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"once_exec_script","arguments":{"path":"build.sh"}}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"once_exec_script","arguments":{"path":"build.sh"}}}
EOF

    When call /bin/sh -c 'cat "$1" | "$2" -C "$3" mcp --allow-run' sh "$WORKSPACE/mcp.jsonl" "$ONCE_BIN" "$WORKSPACE"
    The status should be success
    The stdout should include '"valid":true'
    The stdout should include '"cache":"hit"'
    The stdout should include '"evidence_subject"'
    The path "$WORKSPACE/output.txt" should be file
    The contents of file "$WORKSPACE/output.txt" should equal 'hello from an annotated script'
  End

  It 'verifies scripts and retrieves the same retained report without execution permission'
    cat > "$WORKSPACE/build.sh" <<'SH'
#!/bin/sh
# once output "out.txt"
printf stable > out.txt
SH
    verify_then_query() {
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"once_exec_script","arguments":{"path":"build.sh","verify_reproducible":true}}}' |
        "$ONCE_BIN" -C "$WORKSPACE" mcp --allow-run > "$SPEC_ROOT/verified.json" || return
      id=$(jq -r '.result.structuredContent.result.id' "$SPEC_ROOT/verified.json")
      printf '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"once_query_verification","arguments":{"id":"%s"}}}\n' "$id" |
        "$ONCE_BIN" -C "$WORKSPACE" mcp > "$SPEC_ROOT/queried.json" || return
      jq -S '.result.structuredContent.result' "$SPEC_ROOT/verified.json" > "$SPEC_ROOT/first.json"
      jq -S '.result.structuredContent.result' "$SPEC_ROOT/queried.json" > "$SPEC_ROOT/second.json"
      cmp "$SPEC_ROOT/first.json" "$SPEC_ROOT/second.json" || return
      jq -c '[.schema, .success, .artifacts_match, .trials]' "$SPEC_ROOT/second.json"
    }
    When call verify_then_query
    The status should be success
    The stdout should equal '["once.verification.v1",true,true,2]'
    The path "$WORKSPACE/out.txt" should not be exist
  End

  It 'returns matching retained evidence and diagnostics without evaluating the current graph'
    mkdir -p "$WORKSPACE/.once/out/tests/unit/test"
    cat > "$WORKSPACE/.once/out/tests/unit/test/test_results.json" <<'EOF'
{"schema":"once.test_results.v1","target":"tests/unit","status":"cancelled","runner":{"type":"native","metadata":{}},"summary":{"total":2,"passed":1,"failed":0,"skipped":0,"flaky":1},"cases":[{"id":"other","name":"other","suite":"unit","status":"passed","runner_metadata":{},"attempts":[{"status":"passed"}]},{"id":"retry","name":"retry","suite":"unit","status":"flaky","runner_metadata":{},"attempts":[{"status":"failed","timestamp":900,"failure":{"message":"first failure"},"producer_extension":{"n":7}},{"status":"producer_future_status","timestamp":100,"producer_extension":[3,1]}]}],"artifacts":{"logs":["../../never-read","https://invalid.example/private"],"native_results":[]}}
EOF
    cat > "$SPEC_ROOT/check_retained.py" <<'PY'
import json, os, pathlib, subprocess
root = pathlib.Path(os.environ['WORKSPACE'])
command = [os.environ['ONCE_BIN'], '-C', str(root)]
path = root / '.once/out/tests/unit/test/test_results.json'
raw = path.read_bytes()
retained = json.loads(raw)

def query(case=None, attempt=None, summary=False, error=None):
    args = {'target': 'tests/unit'}
    cli = command + ['--format', 'json', 'query', 'test-results', 'tests/unit']
    if case is not None:
        args['case'] = case
        cli += ['--case', case]
    if attempt is not None:
        args['attempt'] = attempt
        cli += ['--attempt', str(attempt)]
    if summary:
        args['summary_only'] = True
        cli += ['--summary-only']
    direct = subprocess.run(cli, capture_output=True, text=True)
    request = {'jsonrpc': '2.0', 'id': 1, 'method': 'tools/call', 'params': {'name': 'once_query_test_results', 'arguments': args}}
    mcp = subprocess.run(command + ['mcp'], input=json.dumps(request)+'\n', capture_output=True, text=True)
    assert mcp.returncode == 0, mcp.stderr
    response = json.loads(mcp.stdout)['result']
    if error:
        assert direct.returncode != 0 and not direct.stdout
        cli_error = json.loads(direct.stderr.splitlines()[-1])['error']
        mcp_error = response['structuredContent']['error']
        assert response['isError'] and cli_error['code'] == mcp_error['code'] == error
        assert cli_error['diagnostics'] == mcp_error['diagnostics']
        diagnostic = cli_error['diagnostics'][0]
        assert diagnostic['target'] == 'tests/unit' and diagnostic['attribute'] and diagnostic['repairs']
        assert len(direct.stderr) < 20000 and len(mcp.stdout) < 20000
        return
    assert direct.returncode == 0, direct.stderr
    value = json.loads(direct.stdout)
    assert value == response['structuredContent']['result'] == json.loads(response['content'][0]['text'])
    return value

for manifest in (None, 'invalid [ TOML'):
    if manifest is not None:
        (root / 'once.toml').write_text(manifest)
    before = {str(p.relative_to(root)): (p.read_bytes(), p.stat().st_mtime_ns) for p in root.rglob('*') if p.is_file()}
    first = query('retry', 1)
    latest = query('retry')
    assert first['status'] == 'cancelled' and first['case']['status'] == 'flaky'
    assert first['attempt_result'] == retained['cases'][1]['attempts'][0]
    assert latest['attempt_result'] == retained['cases'][1]['attempts'][1]
    assert latest == query('retry', 2) and latest['attempt'] == latest['attempt_count'] == 2
    assert latest['target_artifacts'] == retained['artifacts'] and latest['artifact_availability'] == 'not_checked'
    assert latest['rerun'] == {'argv': ['once', 'test', '--format', 'json', '--', 'tests/unit'], 'scope': 'target', 'configuration': 'current', 'reason': 'retained_evidence_only'}
    assert query('other')['case']['status'] == 'passed'
    assert query() == retained
    assert query(summary=True)['schema'] == 'once.test_results_summary.v1'
    query('missing', error='test_case_not_found')
    query('retry', 3, error='test_attempt_not_found')
    query('retry', 0, error='invalid_test_result_selection')
    query(attempt=1, error='invalid_test_result_selection')
    query('retry', summary=True, error='invalid_test_result_selection')
    query('', error='invalid_test_result_selection')
    after = {str(p.relative_to(root)): (p.read_bytes(), p.stat().st_mtime_ns) for p in root.rglob('*') if p.is_file()}
    assert before == after, (before.keys(), after.keys())
assert path.read_bytes() == raw
path.write_bytes(raw + b'\n')
assert query('retry')['results_digest'] != latest['results_digest']
path.write_bytes(raw + b' ' * (16777217 - len(raw)))
query('retry', error='test_results_too_large')
oversized = json.loads(raw)
oversized['cases'][1]['attempts'][1]['padding'] = 'x' * 1048576
path.write_text(json.dumps(oversized))
query('retry', error='test_case_projection_too_large')
oversized['schema'] = 'invalid' * 200000
path.write_text(json.dumps(oversized))
query('retry', error='invalid_test_results')
path.write_text('[' * 200 + '0' + ']' * 200)
query('retry', error='invalid_test_results')
path.unlink()
query('retry', error='test_results_unavailable')
print('retained CLI/MCP parity, statuses, selectors, diagnostics, and read-only checks passed')
PY
    When call "$SPEC_PYTHON3" "$SPEC_ROOT/check_retained.py"
    The status should be success
    The stdout should equal 'retained CLI/MCP parity, statuses, selectors, diagnostics, and read-only checks passed'
  End

  It 'keeps editing and execution tools out of the default catalog'
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' > "$WORKSPACE/mcp.jsonl"

    When call /bin/sh -c 'cat "$1" | "$2" -C "$3" mcp | jq -r ".result.tools[].name"' sh "$WORKSPACE/mcp.jsonl" "$ONCE_BIN" "$WORKSPACE"
    The status should be success
    The stdout should include 'once_validate_script'
    The stdout should include 'once_validate_workspace'
    The stdout should not include 'once_exec_script'
    The stdout should not include 'once_run_tests'
    The stdout should not include 'once_apply_edit'
  End
End
