defmodule OnceSite.BlogTest do
  use ExUnit.Case, async: true

  alias OnceSite.Blog
  alias OnceSite.Blog.Authors
  alias OnceSite.Blog.MarkdownConverter
  alias OnceSite.Blog.Post

  test "includes the published posts" do
    slugs = Enum.map(Blog.all_posts(), & &1.slug)

    refute slugs == []
    assert "automation-needs-a-git" in slugs
  end

  test "clamps pages beyond the last available page" do
    count = length(Blog.all_posts())
    {posts, meta} = Blog.paginate(%{"page" => "999"})

    assert meta.total_count == count
    assert meta.current_page == meta.total_pages
    assert length(posts) <= 2
    refute posts == []
  end

  test "reports the most recent post date as the last update" do
    assert Blog.last_updated() == hd(Blog.all_posts()).date
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
    assert html =~ ~s(data-part="code-window")
    assert html =~ ~s(data-part="language")
    assert html =~ ~s(data-part="copy-source")
    assert html =~ ~s(data-part="copy-icon")
  end
end
