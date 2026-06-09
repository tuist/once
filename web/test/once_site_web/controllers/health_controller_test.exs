defmodule OnceSiteWeb.HealthControllerTest do
  use OnceSiteWeb.ConnCase

  test "GET /ready", %{conn: conn} do
    conn = get(conn, ~p"/ready")
    assert text_response(conn, 200) == "ok"
  end
end
