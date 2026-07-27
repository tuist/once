defmodule OnceSiteWeb.Docs.MarkdownTest do
  use ExUnit.Case, async: true

  alias OnceSiteWeb.Docs.Markdown

  test "extracts the title and headings" do
    result = Markdown.render("# Getting Started\n\n## First\n\n### Second")

    assert result.title == "Getting Started"
    assert [%{level: 2, id: "first", text: "First"}, %{level: 3, id: "second"}] = result.headings
  end

  test "rewrites internal links under /docs" do
    %{html: html} = Markdown.render("[a](/guide/why) [b](/reference/manifest) [c](https://x.com)")

    assert html =~ ~s(href="/docs/guide/why")
    assert html =~ ~s(href="/docs/reference/manifest")
    assert html =~ ~s(href="https://x.com")
  end

  test "wraps code blocks in the Noora code-window" do
    %{html: html} = Markdown.render("```sh\nls\n```")

    assert html =~ ~s(class="code-window")
    assert html =~ ~s(data-part="language")
    assert html =~ ~s(data-part="copy-source")
  end

  test "renders ::: tip as a Noora alert" do
    %{html: html} = Markdown.render("::: tip Heads Up\nBe precise.\n:::\n")

    assert html =~ ~s(class="noora-alert")
    assert html =~ ~s(data-status="success")
    assert html =~ "Heads Up"
    assert html =~ "Be precise."
  end

  test "renders ::: code-group as tabbed panels" do
    markdown = """
    ::: code-group
    ```python [Python]
    print("hi")
    ```

    ```ruby [Ruby]
    puts "hi"
    ```
    :::
    """

    %{html: html} = Markdown.render(markdown)

    assert html =~ ~s(class="code-group")
    assert html =~ ~s(data-part="tab")
    assert html =~ "Python"
    assert html =~ "Ruby"
  end

  test "wraps tables for the NooraTable hook" do
    %{html: html} = Markdown.render("| a | b |\n| - | - |\n| 1 | 2 |")

    assert html =~ ~s(class="noora-table")
    assert html =~ ~s(phx-hook="NooraTable")
  end

  test "gives headings anchor links" do
    %{html: html} = Markdown.render("## My Section")

    assert html =~ ~s(class="heading-anchor")
    assert html =~ ~s(data-part="hash")
  end

  test "keeps the raw markdown for the copy-page feature" do
    %{markdown: markdown} = Markdown.render("---\nnext: false\n---\n\n# Title\n\nBody")

    assert markdown =~ "# Title"
    refute markdown =~ "next: false"
  end
end
