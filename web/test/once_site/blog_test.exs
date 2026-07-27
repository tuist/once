defmodule OnceSite.BlogTest do
  use ExUnit.Case, async: true

  alias OnceSite.Blog
  alias OnceSite.Blog.Authors
  alias OnceSite.Blog.MarkdownConverter

  test "loads and sorts posts at compile time" do
    posts = Blog.all_posts()

    assert Enum.map(posts, & &1.slug) == [
             "why-we-are-building-once",
             "build-graphs-that-explain-themselves",
             "scripts-are-a-feature"
           ]

    assert Enum.all?(posts, &String.starts_with?(&1.body, "<p>"))
    assert Enum.all?(posts, &(Enum.map(&1.authors, fn author -> author.id end) == ["pedro"]))
  end

  test "paginates the compiled posts with Flop metadata" do
    {first_page, first_meta} = Blog.paginate(%{"page" => "1"})
    {second_page, second_meta} = Blog.paginate(%{"page" => "2"})

    assert length(first_page) == 2
    assert first_meta.current_page == 1
    assert first_meta.next_page == 2
    assert first_meta.total_count == 3
    assert first_meta.total_pages == 2

    assert Enum.map(second_page, & &1.slug) == ["scripts-are-a-feature"]
    assert second_meta.current_page == 2
    assert second_meta.previous_page == 1
  end

  test "normalizes invalid and out-of-range pages" do
    {invalid_page, invalid_meta} = Blog.paginate(%{"page" => "unknown"})
    {last_page, last_meta} = Blog.paginate(%{"page" => "99"})

    assert Enum.map(invalid_page, & &1.slug) == [
             "why-we-are-building-once",
             "build-graphs-that-explain-themselves"
           ]

    assert invalid_meta.current_page == 1
    assert Enum.map(last_page, & &1.slug) == ["scripts-are-a-feature"]
    assert last_meta.current_page == 2
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
