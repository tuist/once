#shellcheck shell=bash

Describe 'activated trigger simulation'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  commit_trigger_workspace() {
    git -C "$WORKSPACE" add .
    git -C "$WORKSPACE" -c user.name=Test -c user.email=test@example.invalid commit -qm "$1"
    REVISION=$(git -C "$WORKSPACE" rev-parse HEAD)
    printf '{"event":"vcs.ref_changed","ref":"refs/heads/main","revision":"%s"}\n' "$REVISION" > "$SPEC_ROOT/event.json"
  }

  create_trigger_workspace() {
    git -C "$WORKSPACE" init -q
    printf '.struo/\n' > "$WORKSPACE/.gitignore"
    mkdir -p "$WORKSPACE/modules" "$WORKSPACE/checks"
    cat > "$WORKSPACE/modules/check.star" <<'EOF'
def _impl(ctx):
    out = declare_output(ctx["capability"] + ".txt")
    write_path(out, _configured_attr(ctx, "message", "missing") + ":" + ctx["configuration"]["os"])
    return {"out": out}

check = target_kind(
    attrs = [attr("message", "string", required = True)],
    capabilities = [capability("build", ["default"]), capability("test", ["default"])],
    impl = _impl,
)
EOF
    cat > "$WORKSPACE/checks/struo.toml" <<'EOF'
[[target]]
name = "core"
kind = "check"
[target.attrs]
message = { select = { unexpected = "wrong", default = "expected" } }
EOF
    cat > "$WORKSPACE/struo.toml" <<'EOF'
[modules]
paths = ["modules/*.star"]

[[trigger]]
id = "verify-main"
on = { event = "vcs.ref_changed", refs = ["refs/heads/main"] }
[[trigger.invoke]]
target = "checks/core"
capability = "build"
configuration = { os = "linux", arch = "x86_64", tokens = [] }
[[trigger.invoke]]
target = "checks/core"
capability = "test"
configuration = { os = "linux", arch = "x86_64", tokens = [] }
EOF
    commit_trigger_workspace initial
    DEFINITION_REVISION=$REVISION
    struo --format json edit activate-triggers --revision "$DEFINITION_REVISION" >/dev/null
  }

  remove_candidate_trigger() {
    cat > "$WORKSPACE/struo.toml" <<'EOF'
[modules]
paths = ["modules/*.star"]
[workspace.configuration]
tokens = ["unexpected"]
EOF
    commit_trigger_workspace candidate
  }

  run_and_inspect_trigger() {
    STRUO_CACHE_PROVIDER=unavailable struo --format json --memory-limit 64MiB run --trigger verify-main --event "$SPEC_ROOT/event.json" > "$SPEC_ROOT/run.json" || {
      cat "$SPEC_ROOT/run.json" >&2
      return 1
    }
    PATH=/usr/bin:/bin python3 - "$SPEC_ROOT/run.json" "$WORKSPACE" "$DEFINITION_REVISION" "$REVISION" <<'PY'
import json, pathlib, sys
run = json.load(open(sys.argv[1]))
assert run['mode'] == 'simulation' and run['status'] == 'passed', run
assert run['memory_limit'] == 64 * 1024 * 1024, run
assert run['selection']['definition_revision'] == sys.argv[3], run
assert run['selection']['observation']['revision'] == sys.argv[4], run
assert [o['result']['capability'] for o in run['outcomes']] == ['build', 'test'], run
for outcome in run['outcomes']:
    assert outcome['result']['outputs'], outcome
    for output in outcome['result']['outputs']:
        assert (pathlib.Path(sys.argv[2]) / output).read_text() == 'expected:linux', output
stored = json.load(open(pathlib.Path(sys.argv[2]) / '.struo/triggers/runs' / (run['id'] + '.json')))
assert stored == run
print('activated obligations and explicit configuration preserved')
PY
  }

  It 'runs the activated obligations after the candidate deletes its trigger without merging candidate defaults'
    create_trigger_workspace
    remove_candidate_trigger

    When call run_and_inspect_trigger
    The status should be success
    The stdout should equal 'activated obligations and explicit configuration preserved'
  End

  It 'does not use a candidate narrowed ref or invocation selection'
    create_trigger_workspace
    cat > "$WORKSPACE/struo.toml" <<'EOF'
[modules]
paths = ["modules/*.star"]
[[trigger]]
id = "verify-main"
on = { event = "vcs.ref_changed", refs = ["refs/heads/other"] }
[[trigger.invoke]]
target = "checks/core"
capability = "build"
configuration = { os = "linux", arch = "x86_64", tokens = [] }
EOF
    commit_trigger_workspace narrowed

    When call run_and_inspect_trigger
    The status should be success
    The stdout should equal 'activated obligations and explicit configuration preserved'
  End

  It 'rejects a dirty checkout before evaluation'
    create_trigger_workspace
    printf '\n# dirty\n' >> "$WORKSPACE/checks/struo.toml"

    When call struo --format json run --trigger verify-main --event "$SPEC_ROOT/event.json"
    The status should equal 2
    The stderr should include 'trigger_dirty_checkout'
    The stdout should be blank
    The path "$WORKSPACE/.struo/out" should not be exist
  End

  It 'rejects a different source revision before evaluation'
    create_trigger_workspace
    remove_candidate_trigger
    printf '{"event":"vcs.ref_changed","ref":"refs/heads/main","revision":"%s"}\n' "$DEFINITION_REVISION" > "$SPEC_ROOT/event.json"

    When call struo --format json run --trigger verify-main --event "$SPEC_ROOT/event.json"
    The status should equal 2
    The stderr should include 'trigger_revision_mismatch'
    The stdout should be blank
  End

  It 'fails the aggregate when a candidate removes an activated target'
    create_trigger_workspace
    rm "$WORKSPACE/checks/struo.toml"
    commit_trigger_workspace removed-target

    When call struo --format json run --trigger verify-main --event "$SPEC_ROOT/event.json"
    The status should equal 1
    The stdout should include '"status":"failed"'
    The stdout should include 'no target matches'
    The path "$WORKSPACE/.struo/out" should not be exist
  End

  It 'checks every selected capability before executing an earlier valid root'
    create_trigger_workspace
    sed -i.bak 's/, capability("test", \["default"\])//' "$WORKSPACE/modules/check.star"
    rm "$WORKSPACE/modules/check.star.bak"
    commit_trigger_workspace removed-capability

    When call struo --format json run --trigger verify-main --event "$SPEC_ROOT/event.json"
    The status should equal 1
    The stdout should include '"status":"failed"'
    The stdout should include 'does not expose'
    The path "$WORKSPACE/.struo/out" should not be exist
  End

  It 'keeps a failed invocation in the aggregate instead of returning green'
    create_trigger_workspace
    cat > "$WORKSPACE/modules/check.star" <<'EOF'
def _impl(ctx):
    if ctx["capability"] == "test":
        run_action(argv = [host_which("false")], outputs = [])
    else:
        out = declare_output("build.txt")
        write_path(out, "built")
    return {}

check = target_kind(
    attrs = [attr("message", "string", required = True)],
    capabilities = [capability("build", ["default"]), capability("test", ["default"])],
    impl = _impl,
)
EOF
    commit_trigger_workspace failed-test

    When call struo --format json run --trigger verify-main --event "$SPEC_ROOT/event.json"
    The status should equal 1
    The stdout should include '"status":"failed"'
    The stdout should include '"status":"passed"'
    The stdout should include 'exit code 1'
  End

  It 'never treats an unmatched event as a successful empty selection'
    create_trigger_workspace
    printf '{"event":"vcs.ref_changed","ref":"refs/heads/other","revision":"%s"}\n' "$REVISION" > "$SPEC_ROOT/event.json"

    When call struo --format json query trigger-plan verify-main --file "$SPEC_ROOT/event.json"
    The status should equal 2
    The stderr should include 'trigger_event_mismatch'
    The stdout should be blank
  End

  It 'inspects and activates trigger metadata without evaluating project modules'
    create_trigger_workspace
    printf 'fail("project code must not run")\n' > "$WORKSPACE/modules/check.star"
    commit_trigger_workspace invalid-project-code

    When call struo --format json edit activate-triggers --revision "$REVISION"
    The status should be success
    The stdout should include '"activated":true'
    The stdout should include 'verify-main'
  End

  It 'rejects activation by a mutable ref'
    create_trigger_workspace

    When call struo --format json edit activate-triggers --revision HEAD
    The status should equal 2
    The stderr should include 'full lowercase commit object id'
    The stdout should be blank
  End

  contend_during_discovery() {
    cat > "$SPEC_ROOT/prefix.star" <<EOF
def _resolve(ctx):
    host_command(["/bin/sh", "-c", "touch '$SPEC_ROOT/discovering'; sleep 1"], cwd = "$WORKSPACE")
    return []
EOF
    cat "$SPEC_ROOT/prefix.star" "$WORKSPACE/modules/check.star" > "$SPEC_ROOT/slow.star"
    sed 's/impl = _impl,/impl = _impl, resolver = _resolve,/' "$SPEC_ROOT/slow.star" > "$WORKSPACE/modules/check.star"
    commit_trigger_workspace slow-discovery
    struo --format json run --trigger verify-main --event "$SPEC_ROOT/event.json" > "$SPEC_ROOT/first.json" 2> "$SPEC_ROOT/first.err" &
    first_pid=$!
    for _ in $(seq 1 100); do
      [ -e "$SPEC_ROOT/discovering" ] && break
      sleep 0.05
    done
    struo --format json edit activate-triggers --revision "$DEFINITION_REVISION"
    competing_status=$?
    wait "$first_pid" || {
      cat "$SPEC_ROOT/first.json" "$SPEC_ROOT/first.err" >&2
      return 1
    }
    return "$competing_status"
  }

  It 'holds the workspace allocation before discovery so activation cannot race an active run'
    create_trigger_workspace

    When call contend_during_discovery
    The status should equal 2
    The stderr should include 'another trigger operation owns this workspace'
    The stdout should be blank
  End
End
