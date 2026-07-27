defmodule OnceSiteWeb.PageControllerTest do
  use OnceSiteWeb.ConnCase

  test "GET /", %{conn: conn} do
    conn = get(conn, ~p"/")
    response = html_response(conn, 200)

    assert response =~ "Build once."
    assert response =~ "Natively supported"
    assert response =~ "Built with"
    assert response =~ "Join Discord"
    assert response =~ "https://discord.gg/fTpB5e3rRp"
    assert response =~ ~s(aria-label="GitHub")
    assert response =~ ~s(aria-label="Discord")
  end
end
