#shellcheck shell=bash
# End-to-end specs for Starlark analysis-backed graph actions.

Describe 'graph declared actions'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  create_declared_action_workspace() {
    mkdir -p "$WORKSPACE/rules" "$WORKSPACE/tools/demo"
    cat > "$WORKSPACE/once.toml" <<'EOF'
[rules]
paths = ["rules/*.star"]
EOF
    cat > "$WORKSPACE/rules/demo.star" <<'EOF'
def _demo_impl(ctx):
    out = declare_output(ctx["capability"] + ".txt")
    content = ctx["attr"]["message"] + ":" + ctx["capability"]
    run_action(
        argv = [host_which("sh"), "-c", "printf \"%s\" \"$1\" > \"$2\"", "sh", content, out],
        outputs = [out],
        identifier = ctx["label"]["id"] + ":" + ctx["capability"],
    )
    return {"out": out}

demo_action = rule(
    docs = "Demo action rule",
    attrs = [
        attr("message", "string", required = True, docs = "Message to emit"),
    ],
    providers = ["demo_output"],
    capabilities = [
        capability("build", ["default"]),
        capability("run", ["default"]),
        capability("test", ["default"]),
    ],
    impl = _demo_impl,
)
EOF
    cat > "$WORKSPACE/tools/demo/once.toml" <<'EOF'
[[target]]
name = "Task"
kind = "demo_action"

[target.attrs]
message = "hello"
EOF
  }

  It 'builds custom rule declared actions through the graph command'
    create_declared_action_workspace

    When call once --format json build tools/demo/Task
    The status should be success
    The stdout should include '"target":"tools/demo/Task"'
    The stdout should include '"capability":"build"'
    The stdout should include '"status":"completed"'
    The stdout should include '.once/out/tools/demo/Task/build.txt'
    The path "$WORKSPACE/.once/out/tools/demo/Task/build.txt" should be file
    The contents of file "$WORKSPACE/.once/out/tools/demo/Task/build.txt" should equal 'hello:build'
  End

  It 'runs custom rule declared actions through the graph command'
    create_declared_action_workspace

    When call once --format json run tools/demo/Task
    The status should be success
    The stdout should include '"target":"tools/demo/Task"'
    The stdout should include '"capability":"run"'
    The stdout should include '"status":"completed"'
    The stdout should include '.once/out/tools/demo/Task/run.txt'
    The path "$WORKSPACE/.once/out/tools/demo/Task/run.txt" should be file
    The contents of file "$WORKSPACE/.once/out/tools/demo/Task/run.txt" should equal 'hello:run'
  End

  It 'tests custom rule declared actions through the graph command'
    create_declared_action_workspace

    When call once --format json test tools/demo/Task
    The status should be success
    The stdout should include '"target":"tools/demo/Task"'
    The stdout should include '"capability":"test"'
    The stdout should include '"status":"completed"'
    The stdout should include '.once/out/tools/demo/Task/test.txt'
    The path "$WORKSPACE/.once/out/tools/demo/Task/test.txt" should be file
    The contents of file "$WORKSPACE/.once/out/tools/demo/Task/test.txt" should equal 'hello:test'
  End
End
