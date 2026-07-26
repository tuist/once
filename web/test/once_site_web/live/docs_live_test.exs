defmodule OnceSiteWeb.DocsLiveTest do
  use OnceSiteWeb.ConnCase

  import Phoenix.LiveViewTest

  test "GET /docs renders the landing page", %{conn: conn} do
    {:ok, _view, html} = live(conn, ~p"/docs")

    assert html =~ "id=\"docs-layout\""
    assert html =~ ~s(data-part="feature-card")
  end

  test "renders a guide page with the docs layout and prose", %{conn: conn} do
    {:ok, _view, html} = live(conn, ~p"/docs/guide/why")

    assert html =~ "id=\"docs-sidebar\""
    assert html =~ "data-prose"
  end

  test "renders a hand-written reference page", %{conn: conn} do
    {:ok, _view, html} = live(conn, ~p"/docs/reference/prelude/apple_library")

    assert html =~ "data-prose"
    assert html =~ "apple_library"
  end

  test "renders a not-found message for unknown pages", %{conn: conn} do
    {:ok, _view, html} = live(conn, ~p"/docs/nope/missing")

    assert html =~ "Page not found"
  end

  test "serves raw markdown", %{conn: conn} do
    conn = get(conn, ~p"/docs-markdown/guide/why")

    assert response_content_type(conn, :md)
    assert response(conn, 200) =~ "#"
  end
end
