defmodule GreetingTest do
  use ExUnit.Case, async: true

  test "formats a greeting" do
    assert Greeting.message("Struo") == "Hello, Struo!"
  end
end
