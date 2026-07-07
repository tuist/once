defmodule GreetingTest do
  use ExUnit.Case, async: true

  test "formats a greeting" do
    assert Greeting.message("Once") == "Hello, Once!"
  end
end
