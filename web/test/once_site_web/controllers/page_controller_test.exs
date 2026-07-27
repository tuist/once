defmodule OnceSiteWeb.PageControllerTest do
  use OnceSiteWeb.ConnCase

  test "GET /", %{conn: conn} do
    conn = get(conn, ~p"/")
    response = html_response(conn, 200)

    assert response =~ "Build once."
    assert response =~ "Natively supported"
    assert response =~ "Built with"
  end
end
