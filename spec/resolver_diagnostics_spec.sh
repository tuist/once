#shellcheck shell=bash

Describe 'owner-scoped resolver diagnostics'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  prepare_missing_dependency_workspace() {
    starter="$REPO_ROOT/crates/once-frontend/prelude/examples/elixir-library-with-mix-dependency"
    cp "$starter/mix.exs" "$starter/mix.lock" "$starter/mix-dependencies.json" "$WORKSPACE/"
    cp -R "$starter/local_helper" "$WORKSPACE/"
    cat > "$WORKSPACE/once.toml" <<'TOML'
[modules]
paths = ["actions.star"]

[[target]]
name = "available"
kind = "file_action"

[[target]]
name = "unavailable"
kind = "mix_dependencies"
srcs = ["mix.exs", "mix.lock", "mix-dependencies.json", "local_helper/mix.exs"]

[target.attrs]
graph_file = "mix-dependencies.json"
mix_env = "prod"

[[target]]
name = "consumer"
kind = "file_action"
deps = ["unavailable"]
TOML
    cat > "$WORKSPACE/actions.star" <<'STAR'
def _build(ctx):
    out = declare_output("output.txt")
    write_path(out, "available\n")
    return {"out": out}

file_action = target_kind(capabilities = [capability("build", ["default"])], impl = _build)
STAR
  }

  It 'builds an unrelated target without fetching dependencies for another project'
    prepare_missing_dependency_workspace
    When call once build available --format json
    The status should be success
    The stdout should include 'available'
    The file "$WORKSPACE/.once/out/available/output.txt" should include 'available'
  End

  It 'keeps missing-source diagnostics and repairs when selecting a dependent target'
    prepare_missing_dependency_workspace
    When call once build consumer --format json
    The status should equal 2
    The stderr should include 'mix_dependency_sources_missing'
    The stderr should include 'mix deps.get --check-locked'
    The stderr should include 'unavailable'
  End
End
