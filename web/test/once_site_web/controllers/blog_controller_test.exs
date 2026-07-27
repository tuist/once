defmodule OnceSiteWeb.BlogControllerTest do
  use OnceSiteWeb.ConnCase

  test "GET /blog renders the first page and pagination", %{conn: conn} do
    response =
      conn
      |> get(~p"/blog")
      |> html_response(200)

    assert response =~ "Ideas for building once"
    assert response =~ "Why we are building Once"
    assert response =~ "Build graphs that explain themselves"
    assert response =~ "Pedro Piñera"
    assert response =~ "gravatar.com/avatar/"
    assert response =~ "Page 1 of 2"
    assert response =~ "Older"
    refute response =~ "Scripts are a feature, not a fallback"
  end

  test "GET /blog?page=2 renders the second page", %{conn: conn} do
    response =
      conn
      |> get(~p"/blog?#{%{page: 2}}")
      |> html_response(200)

    assert response =~ "Scripts are a feature, not a fallback"
    assert response =~ "Page 2 of 2"
    assert response =~ "Newer"
    refute response =~ "Why we are building Once"
  end

  test "GET /blog/:slug renders a post and its metadata", %{conn: conn} do
    response =
      conn
      |> get(~p"/blog/why-we-are-building-once")
      |> html_response(200)

    assert response =~ "Repository automation should become faster"
    assert response =~ "Written by"
    assert response =~ "Pedro Piñera"
    assert response =~ ~s(property="og:type" content="article")
    assert response =~ "/images/og/blog-why-we-are-building-once.jpg"

    assert response =~
             ~s(property="article:published_time" content="2026-07-27T00:00:00Z")
  end

  test "GET /blog/:slug returns not found for an unknown post", %{conn: conn} do
    conn = get(conn, ~p"/blog/unknown")
    assert html_response(conn, 404) =~ "Not Found"
  end

  test "GET /blog/feed.xml returns the full Really Simple Syndication feed", %{conn: conn} do
    conn = get(conn, ~p"/blog/feed.xml")
    response = response(conn, 200)

    assert get_resp_header(conn, "content-type") == ["application/rss+xml; charset=utf-8"]
    assert response =~ "<title>Once blog</title>"
    assert response =~ "<dc:creator>Pedro Piñera</dc:creator>"
    assert response =~ "/blog/why-we-are-building-once"
    assert response =~ "<content:encoded><![CDATA[<p>"
  end

  test "GET /blog/feed.atom returns the full Atom feed", %{conn: conn} do
    conn = get(conn, ~p"/blog/feed.atom")
    response = response(conn, 200)

    assert get_resp_header(conn, "content-type") == ["application/atom+xml; charset=utf-8"]
    assert response =~ "<feed xmlns=\"http://www.w3.org/2005/Atom\">"
    assert response =~ "<name>Pedro Piñera</name>"
    assert response =~ "<published>2026-07-27T00:00:00Z</published>"
  end

  test "marketing image generation includes blog cards" do
    pages = Mix.Tasks.Marketing.Gen.OgImages.pages()
    files = Enum.map(pages, & &1.file)

    assert "blog.jpg" in files
    assert "blog-why-we-are-building-once.jpg" in files
    assert "blog-build-graphs-that-explain-themselves.jpg" in files
    assert "blog-scripts-are-a-feature.jpg" in files

    page = Enum.find(pages, &(&1.file == "blog-why-we-are-building-once.jpg"))
    assert Enum.all?(page.avatars, &String.starts_with?(&1, "https://gravatar.com/avatar/"))
  end
end
