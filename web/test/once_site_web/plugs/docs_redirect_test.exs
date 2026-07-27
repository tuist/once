defmodule OnceSiteWeb.Plugs.DocsRedirectTest do
  use ExUnit.Case, async: true

  import Plug.Conn
  import Plug.Test

  alias OnceSiteWeb.Plugs.DocsRedirect

  test "redirects the legacy docs host to /docs preserving path and query" do
    conn =
      conn(:get, "/guide/why?ref=x")
      |> Map.put(:host, "docs.buildonce.dev")
      |> DocsRedirect.call([])

    assert conn.halted
    assert conn.status == 301
    assert get_resp_header(conn, "location") == ["https://buildonce.dev/docs/guide/why?ref=x"]
  end

  test "passes other hosts through untouched" do
    conn =
      conn(:get, "/docs/guide/why")
      |> Map.put(:host, "buildonce.dev")
      |> DocsRedirect.call([])

    refute conn.halted
  end
end
