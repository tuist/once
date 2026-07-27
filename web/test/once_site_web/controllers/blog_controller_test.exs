defmodule OnceSiteWeb.BlogControllerTest do
  use OnceSiteWeb.ConnCase

  test "GET /blog renders an empty publication", %{conn: conn} do
    response =
      conn
      |> get(~p"/blog")
      |> html_response(200)

    assert response =~ "Ideas for building once"
    assert response =~ "Writing in progress"
    assert response =~ "Posts from the Once team will appear here soon."
    refute response =~ ~s(data-part="blog-entry")
    refute response =~ ~s(data-part="pagination")
  end

  test "GET /blog normalizes pages before the first post", %{conn: conn} do
    response =
      conn
      |> get(~p"/blog?#{%{page: 2}}")
      |> html_response(200)

    assert response =~ "Writing in progress"
    refute response =~ ~s(data-part="pagination")
  end

  test "GET /blog/:slug returns not found for an unknown post", %{conn: conn} do
    conn = get(conn, ~p"/blog/unknown")
    assert html_response(conn, 404) =~ "Not Found"
  end

  test "GET /blog/feed.xml returns an empty Really Simple Syndication feed", %{conn: conn} do
    conn = get(conn, ~p"/blog/feed.xml")
    response = response(conn, 200)

    assert get_resp_header(conn, "content-type") == ["application/rss+xml; charset=utf-8"]
    assert response =~ "<title>Once blog</title>"
    refute response =~ "<item>"
    refute response =~ "<dc:creator>"
  end

  test "GET /blog/feed.atom returns the full Atom feed", %{conn: conn} do
    conn = get(conn, ~p"/blog/feed.atom")
    response = response(conn, 200)

    assert get_resp_header(conn, "content-type") == ["application/atom+xml; charset=utf-8"]
    assert response =~ "<feed xmlns=\"http://www.w3.org/2005/Atom\">"
    refute response =~ "<entry>"
    refute response =~ "<author>"
  end

  test "marketing image generation includes blog cards" do
    pages = Mix.Tasks.Marketing.Gen.OgImages.pages()
    files = Enum.map(pages, & &1.file)

    assert "blog.jpg" in files
    refute Enum.any?(files, &String.starts_with?(&1, "blog-"))
  end
end
