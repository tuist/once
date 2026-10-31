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

  It 'identifies a failed repository checkout without printing credentials or Git output'
    mkdir -p "$SPEC_ROOT/bin"
    cat > "$SPEC_ROOT/bin/git" <<'SH'
#!/bin/sh
printf 'stdout-secret %s\n' "$@"
printf 'stderr-secret %s\n' "$@" >&2
exit 128
SH
    chmod +x "$SPEC_ROOT/bin/git"
    cat > "$WORKSPACE/once.toml" <<'TOML'
[modules]
paths = ["repository.star"]

[[target]]
name = "private-package"
kind = "repository_fixture"
TOML
    cat > "$WORKSPACE/repository-url" <<'URL'
https://username-secret:password-secret@example.invalid/team/private.git?token=query-secret#fragment-secret
URL
    cat > "$WORKSPACE/repository.star" <<'STAR'
def _resolve(ctx):
    root = workspace_root()
    host_git_checkouts([{
        "url": host_file_read(root + "/repository-url").strip(),
        "revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "path": root + "/.once/checkouts/private-package",
    }], cache_root = host_env("XDG_CACHE_HOME") + "/repositories")

repository_fixture = target_kind(resolver = _resolve)
STAR

    When call env PATH="$SPEC_ROOT/bin:$PATH" "$ONCE_BIN" -C "$WORKSPACE" query targets
    The status should be failure
    The stdout should be blank
    The stderr should include 'repository acquisition failed'
    The stderr should include 'https://example.invalid/team/private.git'
    The stderr should include '.once/checkouts/private-package'
    The stderr should include '128'
    The stderr should include 'check repository access and the pinned revision'
    The stderr should not include 'username-secret'
    The stderr should not include 'password-secret'
    The stderr should not include 'query-secret'
    The stderr should not include 'fragment-secret'
    The stderr should not include 'stderr-secret'
    The stderr should not include 'stdout-secret'
    The path "$WORKSPACE/.once/checkouts/private-package" should not be exist
  End
End
