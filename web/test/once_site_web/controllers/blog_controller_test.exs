defmodule OnceSiteWeb.BlogControllerTest do
  use OnceSiteWeb.ConnCase

  test "GET /blog lists the published posts", %{conn: conn} do
    response =
      conn
      |> get(~p"/blog")
      |> html_response(200)

    assert response =~ "Ideas for building once"
    assert response =~ ~s(data-part="blog-entry")
    assert response =~ "Automation needs a git"
    refute response =~ "Writing in progress"
  end

  test "GET /blog clamps out-of-range pages", %{conn: conn} do
    response =
      conn
      |> get(~p"/blog?#{%{page: 999}}")
      |> html_response(200)

    assert response =~ ~s(data-part="blog-entry")
  end

  test "GET /blog/:slug returns not found for an unknown post", %{conn: conn} do
    conn = get(conn, ~p"/blog/unknown")
    assert html_response(conn, 404) =~ "Not Found"
  end

  test "GET /blog/feed.xml returns the Really Simple Syndication feed", %{conn: conn} do
    conn = get(conn, ~p"/blog/feed.xml")
    response = response(conn, 200)

    assert get_resp_header(conn, "content-type") == ["application/rss+xml; charset=utf-8"]
    assert response =~ "<title>Once blog</title>"
    assert response =~ "<item>"
    assert response =~ "<dc:creator>"
    assert response =~ "Automation needs a git"
  end

  test "GET /blog/feed.atom returns the full Atom feed", %{conn: conn} do
    conn = get(conn, ~p"/blog/feed.atom")
    response = response(conn, 200)

    assert get_resp_header(conn, "content-type") == ["application/atom+xml; charset=utf-8"]
    assert response =~ "<feed xmlns=\"http://www.w3.org/2005/Atom\">"
    assert response =~ "<entry>"
    assert response =~ "<author>"
    assert response =~ "Automation needs a git"
  end

  test "marketing image generation includes blog cards" do
    pages = Mix.Tasks.Marketing.Gen.OgImages.pages()
    files = Enum.map(pages, & &1.file)

    assert "blog.jpg" in files
    assert "blog-automation-needs-a-git.jpg" in files
  end
end
