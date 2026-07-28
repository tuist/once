defmodule OnceSite.Blog.MarkdownConverter do
  @moduledoc false

  alias OnceSiteWeb.Docs.HTML

  # NimblePublisher runs this converter while `OnceSite.Blog` compiles, so the
  # web HTML helper must already be compiled. Calling it here at compile time
  # pins it as a compile-time dependency, so it is built first.
  _ = OnceSiteWeb.Docs.HTML.wrap_code_blocks("")

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
                    autolink: true,
                    strikethrough: true,
                    table: true,
                    tasklist: true
                  ],
                  render: [unsafe: true]
                ] ++ @syntax_highlight

  def convert(_path, body, _attributes, _options), do: render(body)

  def render(markdown) do
    markdown
    |> MDEx.to_html!(@mdex_options)
    |> HTML.wrap_code_blocks()
  end
end
