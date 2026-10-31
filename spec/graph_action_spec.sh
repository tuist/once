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

  It 'verifies build artifacts independently of declared logs nested in directory outputs'
    create_declared_action_workspace
    cat > "$WORKSPACE/modules/demo.star" <<'EOF'
def _demo_impl(ctx):
    out = declare_output("bundle")
    run_action(
        argv = [host_which("sh"), "-c", "printf stable > \"$1/artifact\"; printf '%s' \"$$\"", "sh", out],
        outputs = [out],
        stdout = out + "/log.txt",
        create_dirs = [out],
    )
    second = declare_output("second.txt")
    run_action(
        argv = [host_which("sh"), "-c", "printf stable > \"$1\"", "sh", second],
        outputs = [second],
    )
    return {}

demo_action = target_kind(
    attrs = [attr("message", "string")],
    capabilities = [capability("build", ["default"])],
    impl = _demo_impl,
)
EOF
    When call once --format json build --verify-reproducible tools/demo/Task
    The status should be success
    The stdout should include '"artifacts_match":true'
    The stdout should include '"logs_match":false'
    The stdout should include '"log":true'
    The path "$WORKSPACE/.once/out/tools/demo/Task/bundle" should not be exist
  End

  It 'uses declared logs as the verdict when a build declares no other artifacts'
    create_declared_action_workspace
    cat > "$WORKSPACE/modules/demo.star" <<'EOF'
def _demo_impl(ctx):
    log = declare_output("log.txt")
    run_action(argv = [host_which("sh"), "-c", "printf '%s' \"$$\""], outputs = [log], stdout = log)
    return {}

demo_action = target_kind(
    attrs = [attr("message", "string")],
    capabilities = [capability("build", ["default"])],
    impl = _demo_impl,
)
EOF
    When call once --format json build --verify-reproducible tools/demo/Task
    The status should be failure
    The stdout should include '"artifacts_match":null'
    The stdout should include '"logs_match":false'
    The stdout should include '"success":false'
  End

  exercise_cached_environment() {
    create_declared_action_workspace
    cat > "$WORKSPACE/modules/demo.star" <<'EOF'
def _demo_impl(ctx):
    out = declare_output("environment.txt")
    run_action(
        argv = [host_which("sh"), "-c", "printf '%s:%s' \"$ONCE_INHERITED_VALUE\" \"$NATIVE_SETTING\" > \"$1\"", "sh", out],
        outputs = [out],
        inherit_parent_env = True,
        sandbox = "off",
        env = {"NATIVE_SETTING": "resolved"},
    )
    return {}

demo_action = target_kind(
    attrs = [attr("message", "string")],
    capabilities = [capability("build", ["default"])],
    impl = _demo_impl,
)
EOF
    for value in first first second second; do
      ONCE_CHANGE_TRACKER=0 ONCE_INHERITED_VALUE="$value" NATIVE_SETTING=caller once --format json build tools/demo/Task > "$SPEC_ROOT/environment.json" 2> "$SPEC_ROOT/environment.err" || { cat "$SPEC_ROOT/environment.err"; return 1; }
      [ "$(cat "$WORKSPACE/.once/out/tools/demo/Task/environment.txt")" = "$value:resolved" ] || return 1
      jq -r '.cache' "$SPEC_ROOT/environment.json" >> "$SPEC_ROOT/cache-states"
      rm "$WORKSPACE/.once/out/tools/demo/Task/environment.txt"
    done
    [ "$(cat "$SPEC_ROOT/cache-states")" = "$(printf 'miss\nhit\nmiss\nhit')" ] || return 1
    [ -d "$WORKSPACE/.once/analysis" ] || return 1
    if grep -R -E '"ONCE_INHERITED_VALUE"[[:space:]]*:[[:space:]]*"(first|second)"' "$WORKSPACE/.once/analysis"; then
      return 1
    fi
  }

  It 'caches the effective inherited environment, restores outputs, and honors explicit overrides'
    When call exercise_cached_environment
    The status should be success
    The output should equal ''
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
