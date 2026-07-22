defmodule Greeting do
  def message(name) do
    name
    |> LockedGreeting.message()
    |> LocalHelper.decorate()
  end
end
