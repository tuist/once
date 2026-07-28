defmodule OnceSite.RepoTest do
  use ExUnit.Case, async: true

  setup do
    pid = Ecto.Adapters.SQL.Sandbox.start_owner!(OnceSite.Repo, shared: false)
    on_exit(fn -> Ecto.Adapters.SQL.Sandbox.stop_owner(pid) end)
  end

  test "connects to PostgreSQL" do
    assert %{rows: [[1]]} =
             Ecto.Adapters.SQL.query!(OnceSite.Repo, "SELECT 1", [])
  end
end
