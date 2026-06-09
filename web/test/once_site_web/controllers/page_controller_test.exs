defmodule OnceSiteWeb.PageControllerTest do
  use OnceSiteWeb.ConnCase

  test "GET /", %{conn: conn} do
    conn = get(conn, ~p"/")

    assert html_response(conn, 200) =~
             "Once turns scripts into cacheable, remotely executable building blocks"
  end

  test "GET /registry", %{conn: conn} do
    conn = get(conn, ~p"/registry")
    response = html_response(conn, 200)

    assert response =~ "Once scripts and cached commands"
    assert response =~ "release/package"
  end
end
