defmodule OnceSiteWeb.BlogController do
  use OnceSiteWeb, :controller

  alias OnceSite.Blog
  alias OnceSite.Blog.Feed
  alias OnceSiteWeb.Endpoint

  def index(conn, params) do
    {posts, meta} = Blog.paginate(params)

    conn
    |> assign(:page_title, "Blog")
    |> assign(:meta_description, "Ideas and updates from the people building Once.")
    |> assign(:og_image, "/images/og/blog.jpg")
    |> assign(:alternate_feed_title, "Once blog")
    |> assign(:alternate_feed_path, "/blog")
    |> assign(:canonical_url, canonical_url(meta.current_page))
    |> assign(:posts, posts)
    |> assign(:meta, meta)
    |> render(:index)
  end

  def show(conn, %{"slug" => slug}) do
    case Blog.get_post(slug) do
      nil ->
        conn
        |> put_status(:not_found)
        |> put_view(OnceSiteWeb.ErrorHTML)
        |> render(:"404")

      post ->
        conn
        |> assign(:page_title, post.title)
        |> assign(:meta_description, post.description)
        |> assign(:og_type, "article")
        |> assign(:og_image, "/images/og/blog-#{post.slug}.jpg")
        |> assign(:published_time, Date.to_iso8601(post.date) <> "T00:00:00Z")
        |> assign(:authors, post.authors)
        |> assign(:alternate_feed_title, "Once blog")
        |> assign(:alternate_feed_path, "/blog")
        |> assign(:post, post)
        |> render(:show)
    end
  end

  def rss(conn, _params) do
    conn
    |> put_resp_content_type("application/rss+xml")
    |> send_resp(200, Feed.rss(Blog.all_posts()))
  end

  def atom(conn, _params) do
    conn
    |> put_resp_content_type("application/atom+xml")
    |> send_resp(200, Feed.atom(Blog.all_posts()))
  end

  defp canonical_url(1), do: Endpoint.url() <> "/blog"
  defp canonical_url(page), do: Endpoint.url() <> "/blog?page=#{page}"
end
