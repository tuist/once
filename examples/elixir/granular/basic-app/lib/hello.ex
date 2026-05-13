defmodule Hello do
  def main(_argv) do
    IO.puts(Greeting.message("Elixir"))
  end
end
