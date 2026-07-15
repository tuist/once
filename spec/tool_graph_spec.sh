#shellcheck shell=bash
# End-to-end specs for graph tool requirements.

Describe 'graph tools'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_graph_tool_workspace'

  cleanup_graph_tool_workspace() {
    if [ -n "${TOOL_SERVER_PID:-}" ]; then
      kill "$TOOL_SERVER_PID" 2>/dev/null || true
      wait "$TOOL_SERVER_PID" 2>/dev/null || true
      unset TOOL_SERVER_PID
    fi
    cleanup_workspace
  }

  create_graph_tool_workspace() {
    mkdir -p "$WORKSPACE/modules" "$WORKSPACE/tools/probe" "$WORKSPACE/tool-dist"
    cp "$REPO_ROOT/fixtures/tool_graph/probe" "$WORKSPACE/tool-dist/probe"
    chmod +x "$WORKSPACE/tool-dist/probe"

    python3 "$REPO_ROOT/fixtures/tool_graph/http_server.py" \
      "$WORKSPACE/tool-dist" "$WORKSPACE/tool-port" \
      >"$WORKSPACE/tool-server.log" 2>&1 &
    TOOL_SERVER_PID=$!
    while [ ! -s "$WORKSPACE/tool-port" ]; do sleep 0.05; done
    tool_port="$(cat "$WORKSPACE/tool-port")"

    cat > "$WORKSPACE/once.toml" <<'EOF'
[modules]
paths = ["modules/*.star"]
EOF
    cat > "$WORKSPACE/mise.toml" <<EOF
[tools]
"http:once-probe" = { version = "1.0.0", url = "http://127.0.0.1:$tool_port/probe" }
EOF
    cat > "$WORKSPACE/modules/probe.star" <<'EOF'
def _probe_impl(ctx):
    out = declare_output("probe.txt")
    run_action(
        argv = ["probe", out],
        outputs = [out],
        identifier = ctx["label"]["id"] + ":probe",
    )
    return {"out": out}

probe_action = target_kind(
    docs = "Runs a tool supplied by the workspace tool environment.",
    tools = [tool("http:once-probe", executables = ["probe"])],
    capabilities = [capability("build", ["default"])],
    impl = _probe_impl,
)
EOF
    cat > "$WORKSPACE/tools/probe/once.toml" <<'EOF'
[[target]]
name = "Probe"
kind = "probe_action"
EOF

    mise_version="$(once --format json toolchain inspect | sed -n 's/.*"mise_version":"\([^"]*\)".*/\1/p')"
    managed_mise_dir="$XDG_DATA_HOME/once/tools/mise/$mise_version"
    mkdir -p "$managed_mise_dir"
    cp "$(command -v mise)" "$managed_mise_dir/mise"
    chmod +x "$managed_mise_dir/mise"
  }

  It 'installs and exposes a declared mise tool to a graph action'
    create_graph_tool_workspace

    When call env PATH=/usr/bin:/bin "$ONCE_BIN" -C "$WORKSPACE" --format json build tools/probe/Probe
    The status should be success
    The stdout should include '"target":"tools/probe/Probe"'
    The stdout should include '"status":"completed"'
    The path "$XDG_DATA_HOME/once/tools/mise-data/installs/http-once-probe/1.0.0/probe" should be file
    The path "$WORKSPACE/.once/out/tools/probe/Probe/probe.txt" should be file
    The contents of file "$WORKSPACE/.once/out/tools/probe/Probe/probe.txt" should equal 'managed-tool:http:once-probe'
  End
End
