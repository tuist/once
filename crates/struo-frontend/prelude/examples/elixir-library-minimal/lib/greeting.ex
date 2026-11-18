defmodule Greeting do
  def message(name) do
    "Hello, " <> name <> Greeting.Punctuation.mark()
  end
end
