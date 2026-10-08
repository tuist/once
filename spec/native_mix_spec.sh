#shellcheck shell=bash

Describe 'configuration-free native Mix commands'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  mix_tools_unavailable() {
    ! command -v mix >/dev/null 2>&1 || ! command -v elixir >/dev/null 2>&1
  }

  prepare_native_workspace() {
    mkdir -p "$WORKSPACE/.git" "$WORKSPACE/app/lib" "$WORKSPACE/app/test/support" "$WORKSPACE/helper/lib" "$WORKSPACE/app/native"
    printf 'invalid Swift package\n' > "$WORKSPACE/app/native/Package.swift"
    printf 'invalid Swift package\n' > "$WORKSPACE/Package.swift"
    cat > "$WORKSPACE/helper/mix.exs" <<'EX'
defmodule Helper.MixProject do
  use Mix.Project
  def project, do: [app: :helper, version: "0.1.0"]
end
EX
    cat > "$WORKSPACE/helper/lib/helper.ex" <<'EX'
defmodule Helper do
  def value, do: :ok
end
EX
    cat > "$WORKSPACE/app/mix.exs" <<'EX'
defmodule Mix.Tasks.Compile.CheckOnce do
  use Mix.Task.Compiler
  def run(_) do
    marker = Path.join(Mix.Project.app_path(), "priv/compiled")
    if File.exists?(marker), do: raise("project compiler ran twice")
    File.mkdir_p!(Path.dirname(marker))
    File.write!(marker, "compiled")
    {:ok, []}
  end
end

defmodule Mix.Tasks.CheckSetup do
  use Mix.Task
  def run(_) do
    Mix.Task.run("compile")
    unless TestSupport.value() == :test, do: raise("test support was not compiled")
    File.write!("setup-runs.txt", "setup\n", [:append])
  end
end

defmodule App.MixProject do
  use Mix.Project
  def project do
    [app: :app, version: "0.1.0", deps: [{:helper, path: "../helper"}],
     compilers: [:check_once] ++ Mix.compilers(),
     elixirc_paths: if(Mix.env() == :test, do: ["lib", "test/support"], else: ["lib"]),
     aliases: [test: ["check_setup", "test"]]]
  end
end
EX
    printf '1.0.0' > "$WORKSPACE/app/VERSION"
    cat > "$WORKSPACE/app/lib/app.ex" <<'EX'
defmodule App do
  @external_resource "VERSION"
  @version File.read!("VERSION")
  def version, do: @version
  def source_root, do: Path.expand("..", __DIR__)
  def value, do: Helper.value()
end
EX
    cat > "$WORKSPACE/app/lib/dynamic_content.ex" <<'EX'
defmodule DynamicContent do
  def __mix_recompile__?, do: false
end
EX
    cat > "$WORKSPACE/app/test/support/test_support.ex" <<'EX'
defmodule TestSupport do
  def value, do: :test
end
EX
    printf 'ExUnit.start()\n' > "$WORKSPACE/app/test/test_helper.exs"
    cat > "$WORKSPACE/app/test/app_test.exs" <<'EX'
defmodule AppTest do
  use ExUnit.Case, async: true
  test "compiled application and environment-specific support" do
    assert function_exported?(DynamicContent, :__mix_recompile__?, 0)
    assert App.value() == :ok
    assert App.version() == File.read!("VERSION")
    assert App.source_root() == File.cwd!()
    assert TestSupport.value() == :test
  end
  test "native test environment and host tools" do
    assert System.get_env("ONCE_NATIVE_TEST_VALUE") == "present"
    assert System.find_executable("native-test-helper")
  end
end
EX
    mkdir -p "$SPEC_ROOT/host-tools"
    printf '#!/bin/sh\nexit 0\n' > "$SPEC_ROOT/host-tools/native-test-helper"
    chmod +x "$SPEC_ROOT/host-tools/native-test-helper"
  }

  native_once() {
    (cd "$WORKSPACE/app" && PATH="$SPEC_ROOT/host-tools:$PATH" ONCE_NATIVE_TEST_VALUE=present "$ONCE_BIN" --format json "$@")
  }

  exercise_native_commands() {
    native_once build > "$SPEC_ROOT/build.json" 2> "$SPEC_ROOT/build.err" || { cat "$SPEC_ROOT/build.err"; return 1; }
    rm -rf "$WORKSPACE/.once/out"
    native_once build > "$SPEC_ROOT/restored.json" 2> "$SPEC_ROOT/restored.err" || { cat "$SPEC_ROOT/restored.err"; return 1; }
    grep -q '"cache":"hit"' "$SPEC_ROOT/restored.json" || return
    test -f "$WORKSPACE/.once/out/app/mix_application_dev/mix/dev/lib/app/priv/compiled" || return
    MIX_ENV=test native_once build > "$SPEC_ROOT/test-build.json" 2> "$SPEC_ROOT/test-build.err" || { cat "$SPEC_ROOT/test-build.err"; return 1; }
    test -f "$WORKSPACE/.once/out/app/mix_application_test/mix/test/lib/app/ebin/Elixir.TestSupport.beam" || return
    rm -rf "$WORKSPACE/.once/tmp"
    native_once test > "$SPEC_ROOT/test.json" 2> "$SPEC_ROOT/test.err" || { cat "$SPEC_ROOT/test.json" "$SPEC_ROOT/test.err" "$WORKSPACE/.once/out/app/mix_tests/test/elixir-test.log"; return 1; }
    rm -rf "$WORKSPACE/.once/tmp"
    native_once test > "$SPEC_ROOT/retest.json" 2> "$SPEC_ROOT/retest.err" || { cat "$SPEC_ROOT/retest.json" "$SPEC_ROOT/retest.err" "$WORKSPACE/.once/out/app/mix_tests/test/elixir-test.log"; return 1; }
    [ "$(wc -l < "$WORKSPACE/app/setup-runs.txt" | tr -d ' ')" = 2 ] || return
    test ! -f "$WORKSPACE/once.toml" || return
    test ! -f "$WORKSPACE/app/once.toml" || return
    python3 - "$WORKSPACE/.once/out/app/mix_tests/test/test_results.json" <<'PY'
import json, sys
result = json.load(open(sys.argv[1]))
assert result["status"] == "passed", result
assert result["summary"]["total"] == 2, result
assert result["summary"]["passed"] == 2, result
assert result["summary"]["failed"] == 0, result
assert result["artifacts"]["logs"] == [".once/out/app/mix_tests/test/elixir-test.log"], result
PY
  }

  exercise_failing_test() {
    exercise_native_commands || return
    cat > "$WORKSPACE/app/test/failing_test.exs" <<'EX'
defmodule FailingTest do
  use ExUnit.Case, async: true
  test "a real test failure" do
    assert false
  end
end
EX
    native_once test > "$SPEC_ROOT/failed.json" 2> "$SPEC_ROOT/failed.err"
    [ "$?" = 1 ] || return 1
    python3 - "$WORKSPACE/.once/out/app/mix_tests/test/test_results.json" <<'PY'
import json, sys
result = json.load(open(sys.argv[1]))
assert result["status"] == "failed", result
assert result["summary"]["total"] == 3, result
assert result["summary"]["failed"] == 1, result
assert result["summary"]["passed"] == 2, result
PY
  }

  exercise_resource_invalidation() {
    exercise_native_commands || return
    printf '2.0.0' > "$WORKSPACE/app/VERSION"
    native_once test > "$SPEC_ROOT/changed-resource.json" 2> "$SPEC_ROOT/changed-resource.err" || { cat "$SPEC_ROOT/changed-resource.err" "$WORKSPACE/.once/out/app/mix_tests/test/elixir-test.log"; return 1; }
    native_once build > "$SPEC_ROOT/changed-build.json" 2> "$SPEC_ROOT/changed-build.err" || return
    native_once build > "$SPEC_ROOT/unchanged-build.json" 2> "$SPEC_ROOT/unchanged-build.err" || return
    grep -q '"cache":"hit"' "$SPEC_ROOT/unchanged-build.json"
  }

  exercise_structured_counts() {
    cat > "$WORKSPACE/app/test/noise_test.exs" <<'EX'
defmodule NoiseTest do
  use ExUnit.Case
  test "summary-looking output is not a result" do
    IO.puts("7 tests, 3 failures")
    IO.puts("Result: 99 passed, 5 skipped")
  end
  @tag :skip
  test "skipped", do: :ok
end
EX
    native_once test > "$SPEC_ROOT/noise.json" 2> "$SPEC_ROOT/noise.err" || { cat "$SPEC_ROOT/noise.err" "$WORKSPACE/.once/out/app/mix_tests/test/elixir-test.log"; return 1; }
    python3 - "$WORKSPACE/.once/out/app/mix_tests/test/test_results.json" <<'PY'
import json, sys
result = json.load(open(sys.argv[1]))
assert result["status"] == "passed", result
assert result["summary"] == {"total": 4, "passed": 3, "failed": 0, "skipped": 1, "flaky": 0}, result
PY
    [ "$?" = 0 ] || return
    cat > "$WORKSPACE/app/test/invalid_test.exs" <<'EX'
defmodule InvalidTest do
  use ExUnit.Case
  setup_all do
    raise "failed setup invalidates the test"
  end
  test "invalid", do: :ok
end
EX
    native_once test > "$SPEC_ROOT/invalid.json" 2> "$SPEC_ROOT/invalid.err"
    [ "$?" = 1 ] || return 1
    python3 - "$WORKSPACE/.once/out/app/mix_tests/test/test_results.json" <<'PY'
import json, sys
result = json.load(open(sys.argv[1]))
assert result["status"] == "failed", result
assert result["summary"] == {"total": 5, "passed": 3, "failed": 1, "skipped": 1, "flaky": 0}, result
PY
  }

  exercise_source_location() {
    native_once build > "$SPEC_ROOT/location-first.json" 2> "$SPEC_ROOT/location-first.err" || return
    destination="$SPEC_ROOT/other-workspace"
    mkdir -p "$destination/.git"
    cp -R "$WORKSPACE/app" "$WORKSPACE/helper" "$destination/"
    WORKSPACE="$destination"
    native_once build > "$SPEC_ROOT/location-second.json" 2> "$SPEC_ROOT/location-second.err" || return
    (cd "$WORKSPACE/app" && elixir -pa "$WORKSPACE/.once/out/app/mix_application_dev/mix/dev/lib/app/ebin" -e 'unless App.source_root() == File.cwd!(), do: raise("cached bytecode retained a different source directory")')
  }

  It 'partitions native bytecode containing source locations across checkouts'
    Skip if 'Mix toolchain unavailable on this host' mix_tools_unavailable
    prepare_native_workspace
    When call exercise_source_location
    The status should be success
  End

  It 'invalidates compilation when a project-owned external resource changes'
    Skip if 'Mix toolchain unavailable on this host' mix_tools_unavailable
    prepare_native_workspace
    When call exercise_resource_invalidation
    The status should be success
  End

  It 'counts native results from ExUnit events, including skipped and invalid tests'
    Skip if 'Mix toolchain unavailable on this host' mix_tools_unavailable
    prepare_native_workspace
    When call exercise_structured_counts
    The status should be success
  End

  It 'builds and tests the current package with sibling dependencies, aliases, and native environment'
    Skip if 'Mix toolchain unavailable on this host' mix_tools_unavailable
    prepare_native_workspace
    When call exercise_native_commands
    The status should be success
  End

  It 'reports actual failing tests instead of a successful empty invocation'
    Skip if 'Mix toolchain unavailable on this host' mix_tools_unavailable
    prepare_native_workspace
    When call exercise_failing_test
    The status should be success
  End
End
