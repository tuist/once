defmodule OnceSiteWeb.Docs.Markdown do
  @moduledoc """
  Renders documentation markdown into the HTML the Noora docs styles expect,
  plus a title, an on-this-page outline, and the raw source (for "copy page").

  Pipeline: strip frontmatter, expand VitePress containers
  (`OnceSiteWeb.Docs.Containers`), render with MDEx, then post-process the HTML
  with `OnceSiteWeb.Docs.HTML` (alerts, code windows, tables, heading anchors)
  and rewrite internal links under `/docs`.
  """

  alias OnceSiteWeb.Docs.Containers
  alias OnceSiteWeb.Docs.HTML

  @type heading :: %{level: pos_integer(), id: String.t(), text: String.t()}
  @type t :: %{
          title: String.t() | nil,
          html: String.t(),
          headings: [heading()],
          markdown: String.t()
        }

  # Syntax highlighting is disabled in tests for speed and determinism.
  @syntax_highlight (if Mix.env() == :test do
                       [syntax_highlight: nil]
                     else
                       [
                         syntax_highlight: [
                           engine: :lumis,
                           opts: [
                             formatter:
                               {:html_multi_themes,
                                themes: [light: "github_light", dark: "github_dark"],
                                default_theme: "light-dark()"}
                           ]
                         ]
                       ]
                     end)

  @mdex_options [
                  extension: [
                    header_id_prefix: "",
                    table: true,
                    strikethrough: true,
                    tasklist: true,
                    autolink: true,
                    alerts: true
                  ],
                  render: [unsafe: true]
                ] ++ @syntax_highlight

  @doc "Render a markdown document into `%{title:, html:, headings:, markdown:}`."
  @spec render(String.t()) :: t()
  def render(markdown) do
    source = strip_frontmatter(markdown)

    rendered =
      source
      |> Containers.preprocess()
      |> MDEx.to_html!(@mdex_options)

    # Read-only parse for the title and outline; the output stays a string so
    # syntax-highlighting whitespace is never collapsed by Floki.
    tree = Floki.parse_fragment!(rendered)
    title = extract_title(tree)
    headings = extract_headings(tree)

    html =
      rendered
      |> HTML.build_alerts()
      |> HTML.wrap_code_blocks()
      |> HTML.add_heading_anchors()
      |> HTML.rewrite_links()
      |> HTML.wrap_tables()

    %{title: title, html: html, headings: headings, markdown: source}
  end

  defp strip_frontmatter("---\n" <> rest) do
    case String.split(rest, "\n---", parts: 2) do
      [_frontmatter, body] -> String.trim_leading(body, "\n")
      _ -> "---\n" <> rest
    end
  end

  defp strip_frontmatter(markdown), do: markdown

  defp extract_title(tree) do
    case Floki.find(tree, "h1") do
      [node | _] -> node |> Floki.text() |> String.trim()
      [] -> nil
    end
  end

  defp extract_headings(tree) do
    tree
    |> Floki.find("h2, h3, h4")
    |> Enum.map(fn {tag, attrs, _children} = node ->
      text = node |> Floki.text() |> String.trim()
      %{level: level(tag), id: attr(attrs, "id") || slugify(text), text: text}
    end)
  end

  defp level("h2"), do: 2
  defp level("h3"), do: 3
  defp level("h4"), do: 4
  defp level(_), do: 2

  defp attr(attrs, key) do
    Enum.find_value(attrs, fn
      {^key, value} -> value
      _ -> nil
    end)
  end

  defp slugify(text) do
    text
    |> String.downcase()
    |> String.replace(~r/[^a-z0-9]+/u, "-")
    |> String.trim("-")
  end
end
