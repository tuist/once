#shellcheck shell=bash
# End-to-end specs for Starlark analysis-backed graph actions.

Describe 'graph declared actions'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  create_declared_action_workspace() {
    mkdir -p "$WORKSPACE/modules" "$WORKSPACE/tools/demo"
    cat > "$WORKSPACE/once.toml" <<'EOF'
[modules]
paths = ["modules/*.star"]
EOF
    cat > "$WORKSPACE/modules/demo.star" <<'EOF'
def _demo_impl(ctx):
    out = declare_output(ctx["capability"] + ".txt")
    content = ctx["attr"]["message"] + ":" + ctx["capability"]
    run_action(
        argv = [host_which("sh"), "-c", "printf \"%s\" \"$1\" > \"$2\"", "sh", content, out],
        outputs = [out],
        identifier = ctx["label"]["id"] + ":" + ctx["capability"],
    )
    return {"out": out}

demo_action = target_kind(
    docs = "Demo action kind",
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

  create_split_action_workspace() {
    mkdir -p "$WORKSPACE/modules" "$WORKSPACE/tools/split"
    cat > "$WORKSPACE/once.toml" <<'EOF'
[modules]
paths = ["modules/*.star"]
EOF
    cat > "$WORKSPACE/modules/split.star" <<'EOF'
def _split_impl(ctx):
    first = declare_output("first.txt")
    second = declare_output("second.txt")
    run_action(
        argv = [host_which("sh"), "-c", "printf \"%s\" \"$1\" > \"$2\"", "sh", ctx["attr"]["message"], first],
        outputs = [first],
        identifier = ctx["label"]["id"] + ":first",
    )
    run_action(
        argv = [host_which("sh"), "-c", "printf second > \"$1\"", "sh", second],
        outputs = [second],
        identifier = ctx["label"]["id"] + ":second",
        depends_on_prior_actions = False,
    )
    return {"first": first, "second": second}

split_action = target_kind(
    docs = "Split action kind",
    attrs = [
        attr("message", "string", required = True, docs = "Message to emit"),
    ],
    providers = ["split_output"],
    capabilities = [capability("build", ["default"])],
    impl = _split_impl,
)
EOF
    cat > "$WORKSPACE/tools/split/once.toml" <<'EOF'
[[target]]
name = "Task"
kind = "split_action"

[target.attrs]
message = "one"
EOF
  }

  It 'builds custom target kind declared actions through the graph command'
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

  It 'records evidence for graph build declared actions'
    create_declared_action_workspace
    once --format json build tools/demo/Task >/dev/null

    When call once --format json query evidence tools/demo/Task:build
    The status should be success
    The stdout should include '"schema":"once.evidence.v1"'
    The stdout should include '"subject":{"kind":"target","id":"tools/demo/Task","capability":"build"}'
    The stdout should include '"status":"passed"'
    The stdout should include '"cache":"miss"'
    The stdout should include '"exit_code":0'
    The stdout should include '.once/out/tools/demo/Task/build.txt'
    The path "$WORKSPACE/.once/once.sqlite" should be file
  End

  It 'runs custom target kind declared actions through the graph command'
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

  It 'tests custom target kind declared actions through the graph command'
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

  It 'keeps later graph actions cached when they skip prior action digests'
    create_split_action_workspace
    once --format json build tools/split/Task >/dev/null
    sed -i.bak 's/message = "one"/message = "two"/' "$WORKSPACE/tools/split/once.toml"

    When call once --format json build tools/split/Task
    The status should be success
    The stdout should include '"target":"tools/split/Task"'
    The stdout should include '"cache":"miss"'
    The contents of file "$WORKSPACE/.once/out/tools/split/Task/first.txt" should equal 'two'
    The contents of file "$WORKSPACE/.once/out/tools/split/Task/second.txt" should equal 'second'
    evidence=$("$ONCE_BIN" -C "$WORKSPACE" --format json query evidence tools/split/Task:build)
    The variable evidence should include '"cache":"hit"'
  End
End
