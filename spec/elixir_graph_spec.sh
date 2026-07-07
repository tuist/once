#shellcheck shell=bash
# End-to-end specs for Elixir graph targets.

Describe 'elixir graph'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  create_elixir_graph_workspace() {
    cp -R "$REPO_ROOT/crates/once-frontend/prelude/examples/elixir-test-minimal/." "$WORKSPACE/"
    rm -rf "$WORKSPACE/.once"
  }

  create_elixir_graph_workspace_without_mix() {
    create_elixir_graph_workspace
    rm -f "$WORKSPACE/mix.exs"
  }

  create_elixir_dependency_graph_workspace() {
    mkdir -p "$WORKSPACE/macro_helpers/lib" "$WORKSPACE/greeting/lib"
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "macro_helpers"
kind = "elixir_library"
srcs = ["macro_helpers/lib/**/*.ex"]

[target.attrs]
app_name = "macro_helpers"

[[target]]
name = "greeting"
kind = "elixir_library"
srcs = ["greeting/lib/**/*.ex"]
deps = ["./macro_helpers"]

[target.attrs]
app_name = "greeting"
TOML
    cat > "$WORKSPACE/macro_helpers/lib/message_macro.ex" <<'EX'
defmodule MessageMacro do
  defmacro defmessage(name, value) do
    quote do
      def unquote(name)(), do: unquote(value)
    end
  end
end
EX
    cat > "$WORKSPACE/greeting/lib/greeting.ex" <<'EX'
defmodule Greeting do
  require MessageMacro

  MessageMacro.defmessage(:message, "hello")
end
EX
  }

  create_elixir_same_target_macro_workspace() {
    mkdir -p "$WORKSPACE/lib"
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "greeting"
kind = "elixir_library"
srcs = ["lib/**/*.ex"]

[target.attrs]
app_name = "greeting"
TOML
    cat > "$WORKSPACE/lib/message_macro.ex" <<'EX'
defmodule MessageMacro do
  defmacro defmessage(name, value) do
    quote do
      def unquote(name)(), do: unquote(value)
    end
  end
end
EX
    cat > "$WORKSPACE/lib/greeting.ex" <<'EX'
defmodule Greeting do
  require MessageMacro

  MessageMacro.defmessage(:message, "hello")
end
EX
  }

  create_elixir_test_support_workspace() {
    mkdir -p "$WORKSPACE/lib" "$WORKSPACE/test/support"
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "greeting"
kind = "elixir_library"
srcs = ["lib/**/*.ex"]

[target.attrs]
app_name = "greeting"
mix_env = "test"

[[target]]
name = "greeting_test"
kind = "elixir_test"
srcs = ["test/**/*_test.exs"]
deps = ["./greeting"]
TOML
    cat > "$WORKSPACE/lib/greeting.ex" <<'EX'
defmodule Greeting do
  def ok, do: :ok
end
EX
    cat > "$WORKSPACE/test/support/message.ex" <<'EX'
defmodule TestMessage do
  def value, do: "one"
end
EX
    cat > "$WORKSPACE/test/test_helper.exs" <<'EX'
ExUnit.start()
Code.require_file("support/message.ex", __DIR__)
EX
    cat > "$WORKSPACE/test/greeting_test.exs" <<'EX'
defmodule GreetingTest do
  use ExUnit.Case

  test "loads support" do
    assert String.length(TestMessage.value()) == 3
  end
end
EX
  }

  create_elixir_external_resource_workspace() {
    mkdir -p "$WORKSPACE/lib"
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "app"
kind = "elixir_library"
srcs = ["lib/**/*.ex"]

[target.attrs]
app_name = "app"
TOML
    printf 'one\n' > "$WORKSPACE/message.txt"
    cat > "$WORKSPACE/lib/app.ex" <<'EX'
defmodule App do
  @external_resource "message.txt"
  @message File.read!("message.txt") |> String.trim()
  def message, do: @message
end
EX
  }

  create_elixir_compile_env_workspace() {
    mkdir -p "$WORKSPACE/lib" "$WORKSPACE/config"
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "app"
kind = "elixir_library"
srcs = ["lib/**/*.ex"]

[target.attrs]
app_name = "app"
config = ["config/**/*.exs"]
TOML
    cat > "$WORKSPACE/config/config.exs" <<'EXS'
import Config

config :app, message: "one"
EXS
    cat > "$WORKSPACE/lib/app.ex" <<'EX'
defmodule App do
  @message Application.compile_env(:app, :message)
  def message, do: @message
end
EX
  }

  create_elixir_recompile_callback_workspace() {
    mkdir -p "$WORKSPACE/lib" "$WORKSPACE/messages"
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "app"
kind = "elixir_library"
srcs = ["lib/**/*.ex"]

[target.attrs]
app_name = "app"
TOML
    printf 'one\n' > "$WORKSPACE/messages/one.txt"
    cat > "$WORKSPACE/lib/app.ex" <<'EX'
defmodule App do
  @paths Path.wildcard("messages/*.txt")
  @paths_hash :erlang.md5(@paths)

  for path <- @paths do
    @external_resource path
  end

  def count, do: unquote(length(@paths))
  def __mix_recompile__?, do: Path.wildcard("messages/*.txt") |> :erlang.md5() != @paths_hash
end
EX
  }

  create_elixir_protocol_workspace() {
    mkdir -p "$WORKSPACE/lib"
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "app"
kind = "elixir_library"
srcs = ["lib/**/*.ex"]

[target.attrs]
app_name = "app"
TOML
    cat > "$WORKSPACE/lib/size.ex" <<'EX'
defprotocol Size do
  def size(value)
end

defmodule Box do
  defstruct [:items]
end

defimpl Size, for: Box do
  def size(box), do: length(box.items)
end
EX
  }

  create_elixir_warning_workspace() {
    mkdir -p "$WORKSPACE/lib"
    cat > "$WORKSPACE/once.toml" <<'TOML'
[[target]]
name = "app"
kind = "elixir_library"
srcs = ["lib/**/*.ex"]

[target.attrs]
app_name = "app"
TOML
    cat > "$WORKSPACE/lib/app.ex" <<'EX'
defmodule App do
  def message do
    unused = :value
    :ok
  end
end
EX
  }

  elixir_eval() {
    elixir -pa "$WORKSPACE/.once/out/app/ebin" -e "$1"
  }

  It 'runs Elixir tests through the graph command without Mix compilation'
    create_elixir_graph_workspace_without_mix

    When call once --format json test greeting_test
    The status should be success
    The stdout should include '"target":"greeting_test"'
    The stdout should include '"capability":"test"'
    The stdout should include '"cache":"miss"'
    The path "$WORKSPACE/.once/out/greeting/ebin/Elixir.Greeting.beam" should be file
    The path "$WORKSPACE/.once/out/greeting/ebin/Elixir.Greeting.Punctuation.beam" should be file
    The path "$WORKSPACE/.once/out/greeting/ebin/greeting.app" should be file
    The contents of file "$WORKSPACE/.once/out/greeting_test/test/elixir-test.log" should include 'Result: 1 passed'
    The contents of file "$WORKSPACE/.once/out/greeting_test/test/elixir-test.log" should not include 'Compiling'
  End

  It 'reuses unchanged whole-target Elixir compile actions'
    create_elixir_graph_workspace
    once --format json test greeting_test >/dev/null
    once --format json test greeting_test >/dev/null

    When call sh -c '"$ONCE_BIN" -C "$WORKSPACE" --format json query evidence greeting:build | jq -e '"'"'any(.[]; .cache == "hit" and (.outputs | keys[] | contains("/modules")))'"'"' >/dev/null'
    The status should be success
    The contents of file "$WORKSPACE/.once/out/greeting_test/test/elixir-test.log" should include 'Result: 1 passed'
  End

  It 'reruns whole-target Elixir compilation after a source changes'
    create_elixir_graph_workspace
    once --format json test greeting_test >/dev/null
    sed -i.bak 's/"Hello, "/"Hi, "/' "$WORKSPACE/lib/greeting.ex"
    sed -i.bak 's/Hello, Once!/Hi, Once!/' "$WORKSPACE/test/greeting_test.exs"

    When call once --format json test greeting_test
    The status should be success
    The stdout should include '"cache":"miss"'
    The contents of file "$WORKSPACE/.once/out/greeting_test/test/elixir-test.log" should include 'Result: 1 passed'
  End

  It 'builds same-target compile-time Elixir macros through one compile action'
    create_elixir_same_target_macro_workspace

    When call once --format json build greeting
    The status should be success
    The stdout should include '"target":"greeting"'
    The path "$WORKSPACE/.once/out/greeting/ebin/Elixir.MessageMacro.beam" should be file
    The path "$WORKSPACE/.once/out/greeting/ebin/Elixir.Greeting.beam" should be file
    The path "$WORKSPACE/_build" should not be exist
  End

  It 'builds compile-time Elixir dependencies through Once actions'
    create_elixir_dependency_graph_workspace

    When call once --format json build greeting
    The status should be success
    The stdout should include '"target":"greeting"'
    The path "$WORKSPACE/.once/out/macro_helpers/ebin/Elixir.MessageMacro.beam" should be file
    The path "$WORKSPACE/.once/out/greeting/ebin/Elixir.Greeting.beam" should be file
    The path "$WORKSPACE/_build" should not be exist
  End

  It 'invalidates tests when support files loaded by test_helper change'
    create_elixir_test_support_workspace
    once --format json test greeting_test >/dev/null
    cat > "$WORKSPACE/test/support/message.ex" <<'EX'
defmodule TestMessage do
  def value, do: "two"
end
EX

    When call once --format json test greeting_test
    The status should be success
    The stdout should include '"cache":"miss"'
    The contents of file "$WORKSPACE/.once/out/greeting_test/test/elixir-test.log" should include 'Result: 1 passed'
  End

  It 'invalidates Elixir compilation when external resources change'
    create_elixir_external_resource_workspace
    once --format json build app >/dev/null
    once --format json build app >/dev/null
    value_before=$(elixir_eval 'IO.write(App.message())')
    printf 'two\n' > "$WORKSPACE/message.txt"

    When call once --format json build app
    The status should be success
    The stdout should include '"target":"app"'
    The variable value_before should equal one
    value_after=$(elixir_eval 'IO.write(App.message())')
    The variable value_after should equal two
    The contents of file "$WORKSPACE/.once/out/app/compile-metadata.tsv" should include $'external\tmessage.txt'
  End

  It 'loads config and invalidates Elixir compilation when compile env changes'
    create_elixir_compile_env_workspace
    once --format json build app >/dev/null
    once --format json build app >/dev/null
    value_before=$(elixir_eval 'IO.write(App.message())')
    cat > "$WORKSPACE/config/config.exs" <<'EXS'
import Config

config :app, message: "two"
EXS

    When call once --format json build app
    The status should be success
    The stdout should include '"target":"app"'
    The variable value_before should equal one
    value_after=$(elixir_eval 'IO.write(App.message())')
    The variable value_after should equal two
    The contents of file "$WORKSPACE/.once/out/app/compile-metadata.tsv" should include 'compile_env'
  End

  It 'reruns Elixir compilation when __mix_recompile__? returns true'
    create_elixir_recompile_callback_workspace
    once --format json build app >/dev/null
    once --format json build app >/dev/null
    value_before=$(elixir_eval 'IO.write(App.count())')
    printf 'two\n' > "$WORKSPACE/messages/two.txt"

    When call once --format json build app
    The status should be success
    The stdout should include '"target":"app"'
    The variable value_before should equal 1
    value_after=$(elixir_eval 'IO.write(App.count())')
    The variable value_after should equal 2
    The contents of file "$WORKSPACE/.once/out/app/compile-metadata.tsv" should include $'recompile\tElixir.App'
  End

  It 'stages consolidated protocol bytecode into Elixir applications'
    create_elixir_protocol_workspace

    When call once --format json build app
    The status should be success
    The stdout should include '"target":"app"'
    The path "$WORKSPACE/.once/out/app/consolidated/Elixir.Size.beam" should be file
    The path "$WORKSPACE/.once/out/app/ebin/Elixir.Size.beam" should be file
    protocol_value=$(elixir_eval 'IO.write(Size.size(%Box{items: [1, 2, 3]}))')
    The variable protocol_value should equal 3
  End

  It 'persists Elixir compile warnings alongside compile metadata'
    create_elixir_warning_workspace

    When call once --format json build app
    The status should be success
    The stdout should include '"target":"app"'
    The contents of file "$WORKSPACE/.once/out/app/compile-warnings.log" should include 'unused'
    The contents of file "$WORKSPACE/.once/out/app/compile-metadata.tsv" should include 'warning'
  End
End
