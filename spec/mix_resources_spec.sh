#shellcheck shell=bash

Describe 'Mix dependency compiler resources'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  mix_tools_unavailable() {
    ! command -v mix >/dev/null 2>&1 || ! command -v elixir >/dev/null 2>&1
  }

  prepare_resource_dependency() {
    cp -R "$REPO_ROOT/crates/once-frontend/prelude/examples/elixir-library-with-mix-dependency/." "$WORKSPACE/"
    mkdir -p "$WORKSPACE/deps/locked_greeting/priv"
    printf 'static' > "$WORKSPACE/deps/locked_greeting/priv/static.txt"
    cat > "$WORKSPACE/deps/locked_greeting/mix.exs" <<'EOF'
defmodule Mix.Tasks.Compile.Resource do
  use Mix.Task.Compiler

  def run(_args) do
    path = Path.join(Mix.Project.app_path(), "priv/native/generated.txt")
    File.mkdir_p!(Path.dirname(path))
    File.write!(path, "generated")
    {:ok, []}
  end
end

defmodule LockedGreeting.MixProject do
  use Mix.Project

  def project do
    [
      app: :locked_greeting,
      version: "0.1.0",
      elixir: "~> 1.14",
      compilers: [:resource] ++ Mix.compilers()
    ]
  end

  def application, do: []
end
EOF
  }

  build_and_restore_resources() {
    once --format json build greeting > "$WORKSPACE/build.json" || return
    app_dir="$WORKSPACE/.once/out/mix-locked-greeting/mix/prod/lib/locked_greeting"
    [ "$(cat "$app_dir/priv/native/generated.txt")" = generated ] || return
    if [ "${1:-}" != generated-only ]; then
      [ "$(cat "$app_dir/priv/static.txt")" = static ] || return
    fi
    rm -rf "$WORKSPACE/.once/out"
    once --format json build greeting > "$WORKSPACE/restored.json" || return
    [ "$(cat "$app_dir/priv/native/generated.txt")" = generated ] || return
    if [ "${1:-}" != generated-only ]; then
      [ "$(cat "$app_dir/priv/static.txt")" = static ] || return
    fi
    grep -q '"cache":"hit"' "$WORKSPACE/restored.json"
  }

  It 'preserves generated and static priv resources across cached builds'
    Skip if 'Mix toolchain unavailable on this host' mix_tools_unavailable
    prepare_resource_dependency

    When call build_and_restore_resources
    The status should be success
  End

  It 'captures generated priv resources without static source resources'
    Skip if 'Mix toolchain unavailable on this host' mix_tools_unavailable
    prepare_resource_dependency
    rm -rf "$WORKSPACE/deps/locked_greeting/priv"

    When call build_and_restore_resources generated-only
    The status should be success
  End
End
