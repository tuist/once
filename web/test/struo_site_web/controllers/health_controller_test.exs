defmodule StruoSiteWeb.HealthControllerTest do
  use StruoSiteWeb.ConnCase

  test "GET /ready", %{conn: conn} do
    conn = get(conn, ~p"/ready")
    assert text_response(conn, 200) == "ok"
  end
end
