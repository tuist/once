defmodule OnceSite.BlogTest do
  use ExUnit.Case, async: true

  alias OnceSite.Blog
  alias OnceSite.Blog.Authors
  alias OnceSite.Blog.MarkdownConverter
  alias OnceSite.Blog.Post

  test "starts without placeholder posts" do
    assert Blog.all_posts() == []
  end

  test "paginates an empty publication" do
    {posts, meta} = Blog.paginate(%{"page" => "2"})

    assert posts == []
    assert meta.current_page == 1
    assert meta.total_count == 0
  end

  test "uses today as the feed date before the first post" do
    assert Blog.last_updated() == Date.utc_today()
  end

  test "builds post metadata from Markdown frontmatter" do
    post =
      Post.build(
        "2026/07-27-an-example.md",
        %{title: "An example", description: "A description", authors: ["pedro"]},
        "<p>Body</p>"
      )

    assert post.slug == "an-example"
    assert post.date == ~D[2026-07-27]
    assert Enum.map(post.authors, & &1.id) == ["pedro"]
    assert post.reading_minutes == 1
  end

  test "builds a Gravatar URL without exposing the email address" do
    author = Authors.fetch!("pedro")
    url = Authors.avatar_url(author, 128)

    assert url =~ "https://gravatar.com/avatar/"
    assert url =~ "s=128"
    refute url =~ author.email
  end

  test "renders Markdown with inline code and Noora code windows" do
    html =
      MarkdownConverter.render("""
      Run `once exec` directly.

      ```sh
      once exec -- ./scripts/build
      ```
      """)

    assert html =~ "<code>once exec</code>"
    assert html =~ ~s(class="code-window")
    assert html =~ ~s(data-part="language")
    assert html =~ ~s(data-part="copy-source")
    assert html =~ ~s(data-part="copy-icon")
  end
end
