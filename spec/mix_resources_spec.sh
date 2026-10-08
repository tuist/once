#shellcheck shell=bash

Describe 'Mix compiler resources'
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

  prepare_build_root_resources() {
    mkdir -p "$WORKSPACE/lib" "$WORKSPACE/helper/lib"
    cat > "$WORKSPACE/mix.exs" <<'EX'
defmodule Mix.Tasks.Compile.Resources do
  use Mix.Task.Compiler
  def run(_args) do
    destination = Path.join(Mix.Project.build_path(), "generated-assets/project/entry.js")
    File.mkdir_p!(Path.dirname(destination))
    File.write!(destination, "generated")
    counter = case File.read("compile-count") do
      {:ok, contents} -> String.to_integer(contents)
      _ -> 0
    end
    File.write!("compile-count", Integer.to_string(counter + 1))
    {:ok, []}
  end
end

defmodule Mix.Tasks.Assets.Validate do
  use Mix.Task
  def run(_args) do
    root = Mix.Project.build_path()
    unless File.read!(Path.join(root, "generated-assets/project/entry.js")) == "generated", do: raise("missing compiler resource")
    unless File.exists?(Path.join(root, "lib/helper/ebin/Elixir.Helper.beam")), do: raise("missing staged dependency")
  end
end

defmodule ResourceProject.MixProject do
  use Mix.Project
  def project, do: [app: :resource_project, version: "0.1.0", compilers: [:resources] ++ Mix.compilers(), deps: [{:helper, path: "helper"}]]
  def application, do: [extra_applications: [:logger]]
end
EX
    printf 'defmodule ResourceProject do\n  def value, do: Helper.value()\nend\n' > "$WORKSPACE/lib/resource_project.ex"
    printf 'defmodule Helper do\n  def value, do: :ok\nend\n' > "$WORKSPACE/helper/lib/helper.ex"
    cat > "$WORKSPACE/helper/mix.exs" <<'EX'
defmodule Helper.MixProject do
  use Mix.Project
  def project, do: [app: :helper, version: "0.1.0"]
end
EX
    printf '%%{}\n' > "$WORKSPACE/mix.lock"
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "dependencies"
kind = "mix_dependencies"
srcs = ["mix.exs", "mix.lock"]
[target.attrs]
mix_env = "prod"

[[target]]
name = "application"
kind = "mix_project"
srcs = ["lib/**/*.ex", "mix.exs"]
deps = ["dependencies"]
[target.attrs]
app_name = "resource_project"
mix_env = "prod"

[[target]]
name = "release"
kind = "mix_release"
deps = ["application"]
[target.attrs]
pre_tasks = [["assets.validate"]]
cacheable = false
TOML
  }

  release_and_restore_build_root_resources() {
    once build release --format json > "$SPEC_ROOT/release-first.json" || return
    rm -rf "$WORKSPACE/.once/out" "$WORKSPACE/.once/tmp"
    once build release --format json > "$SPEC_ROOT/release-restored.json" || return
    [ "$(cat "$WORKSPACE/compile-count")" = 1 ]
  }

  It 'stages compiler-generated build-root resources and dependencies for uncached release tasks'
    Skip if 'Mix toolchain unavailable on this host' mix_tools_unavailable
    prepare_build_root_resources
    When call release_and_restore_build_root_resources
    The status should be success
  End

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
