# shellcheck shell=bash

Describe 'command exit policies'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  create_exit_policy_workspace() {
    mkdir -p "$WORKSPACE/modules"
    printf '[modules]\npaths = ["modules/*.star"]\n' > "$WORKSPACE/struo.toml"
    cat > "$WORKSPACE/modules/policy.star" <<'EOF'
def _impl(ctx):
    run_action(
        argv = [host_which("sh"), "-c", "exit " + str(ctx["attr"]["code"])],
        success_exit_codes = [7],
        cacheable = ctx["attr"]["cacheable"],
    )
    return {"valid": True}

exit_policy = target_kind(
    docs = "Exit policy regression fixture",
    attrs = [
        attr("code", "int", required = True),
        attr("cacheable", "bool", required = True),
    ],
    capabilities = [capability("build", [])],
    impl = _impl,
)
EOF
    mkdir -p "$WORKSPACE/task"
    printf '[[target]]\nname = "Policy"\nkind = "exit_policy"\n[target.attrs]\ncode = %s\ncacheable = %s\n' \
      "$1" "$2" > "$WORKSPACE/task/struo.toml"
  }

  build_policy_twice() {
    if struo build task/Policy > "$SPEC_ROOT/first.out" 2> "$SPEC_ROOT/first.err"; then
      printf 'first=success\n'
    else
      printf 'first=failure\n'
    fi
    struo "$@" build task/Policy
  }

  It 'rejects an excluded zero on repeated cacheable builds'
    create_exit_policy_workspace 0 true

    When call build_policy_twice
    The stdout should include 'first=failure'
    The status should be failure
    The stderr should include 'code 0, which is excluded by success_exit_codes'
  End

  It 'rejects an excluded zero in uncached graph actions'
    create_exit_policy_workspace 0 false

    When call struo build task/Policy
    The status should be failure
    The stderr should include 'code 0, which is excluded by success_exit_codes'
  End

  It 'reuses accepted nonzero-only policies on subsequent builds'
    create_exit_policy_workspace 7 true
    When call build_policy_twice --format json
    The stdout should include 'first=success'
    The status should be success
    The stdout should include '"status":"completed"'
    The stdout should include '"cache":"hit"'
  End
End
