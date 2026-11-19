defmodule StruoSiteWeb.Plugs.LegacyHostRedirectTest do
  use ExUnit.Case, async: true

  import Plug.Conn
  import Plug.Test

  alias StruoSiteWeb.Plugs.LegacyHostRedirect

  test "redirects the legacy docs host to /docs preserving path and query" do
    conn =
      conn(:get, "/guide/why?ref=x")
      |> Map.put(:host, "docs.buildonce.dev")
      |> LegacyHostRedirect.call([])

    assert conn.halted
    assert conn.status == 301
    assert get_resp_header(conn, "location") == ["https://struo.run/docs/guide/why?ref=x"]
  end

  test "redirects the legacy site host to struo.run preserving the path" do
    conn =
      conn(:get, "/docs/guide/why")
      |> Map.put(:host, "buildonce.dev")
      |> LegacyHostRedirect.call([])

    assert conn.status == 301
    assert get_resp_header(conn, "location") == ["https://struo.run/docs/guide/why"]
  end

  test "passes the current host through untouched" do
    conn =
      conn(:get, "/docs/guide/why")
      |> Map.put(:host, "struo.run")
      |> LegacyHostRedirect.call([])

    refute conn.halted
  end
end
