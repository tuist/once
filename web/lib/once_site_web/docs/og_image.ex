defmodule OnceSiteWeb.Docs.OgImage do
  @moduledoc """
  Renders a documentation Open Graph card as a self-contained HTML page.

  The HTML is screenshotted to a JPG at build time by
  `Mix.Tasks.Docs.Gen.OgImages` via headless Chrome. Fonts and the logo are
  embedded as data URIs so the page renders with no network access.
  """
  use Phoenix.Component

  alias Phoenix.HTML.Safe

  @max_title_length 60
  @max_description_length 120
  @width 1920
  @height 1080

  def width, do: @width
  def height, do: @height

  attr :title, :string, required: true
  attr :description, :string, default: nil
  attr :category, :string, default: "Docs"
  attr :subtitle, :string, default: "Docs"
  attr :avatars, :list, default: []
  attr :font_data_uri, :string, required: true
  attr :logo_data_uri, :string, required: true

  def card(assigns) do
    ~H"""
    <html>
      <head>
        <meta charset="utf-8" />
        <style>
          @font-face {
            font-family: 'Inter Variable';
            font-style: normal;
            font-weight: 100 900;
            src: url(<%= @font_data_uri %>) format('woff2');
          }
          * { margin: 0; padding: 0; box-sizing: border-box; }
          html, body {
            width: <%= @image_width %>px;
            height: <%= @image_height %>px;
            overflow: hidden;
            font-family: 'Inter Variable', sans-serif;
            color-scheme: light;
            background: linear-gradient(180deg, #f4f5fe 0%, #efe8ff 100%);
          }
          body {
            padding: 67px;
            display: grid;
            grid-template-rows: minmax(0, 1fr) 80px;
            gap: 80px;
          }
          [data-part="content"] {
            min-height: 0;
            width: 1383px;
            max-width: 100%;
            display: flex;
            flex-direction: column;
            justify-content: center;
            gap: 48px;
          }
          [data-part="title"], [data-part="description"] {
            flex-shrink: 0;
            display: -webkit-box;
            -webkit-box-orient: vertical;
            -webkit-line-clamp: 3;
            overflow: hidden;
            overflow-wrap: anywhere;
            font-weight: 500;
          }
          [data-part="title"] {
            font-size: 128px;
            letter-spacing: -6.4px;
            color: #171a1c;
            line-height: 1.1;
          }
          [data-part="description"] {
            font-size: 64px;
            letter-spacing: -3.2px;
            color: #4e575f;
            line-height: 1.25;
          }
          [data-part="footer"], [data-part="brand"], [data-part="author-meta"] {
            display: flex;
            align-items: center;
            gap: 24px;
          }
          [data-part="footer"] { min-width: 0; justify-content: space-between; }
          [data-part="brand"] { flex-shrink: 0; }
          [data-part="logo"], [data-part="author-avatar"] { width: 80px; height: 80px; }
          [data-part="wordmark"], [data-part="subtitle"], [data-part="category"] {
            font-size: 59px;
            font-weight: 500;
            letter-spacing: -2.9px;
            line-height: 80px;
            white-space: nowrap;
          }
          [data-part="wordmark"], [data-part="subtitle"] {
            background: linear-gradient(92deg, #000 6%, #6a7581 109%);
            -webkit-background-clip: text;
            -webkit-text-fill-color: transparent;
          }
          [data-part="divider"] { width: 3px; height: 80px; background: #c0c8cf; }
          [data-part="author-meta"] { min-width: 0; justify-content: flex-end; }
          [data-part="category"] {
            min-width: 0;
            color: #171a1c;
            overflow: hidden;
            text-overflow: ellipsis;
          }
          [data-part="author-avatars"] {
            display: flex;
            flex-shrink: 0;
            flex-direction: row-reverse;
          }
          [data-part="author-avatar"] {
            margin-right: -16px;
            border: 4px solid #f4f5fe;
            border-radius: 50%;
            background: #e4e7ec;
            object-fit: cover;
          }
          [data-part="author-avatar"]:last-child { margin-right: 0; }
        </style>
      </head>
      <body>
        <main data-part="content">
          <div data-part="title">{truncate(@title, @max_title_length)}</div>
          <div :if={@description} data-part="description">
            {truncate(@description, @max_description_length)}
          </div>
        </main>
        <footer data-part="footer">
          <div data-part="brand">
            <img data-part="logo" src={@logo_data_uri} />
            <div data-part="wordmark">Once</div>
            <div :if={@subtitle} data-part="divider"></div>
            <div :if={@subtitle} data-part="subtitle">{@subtitle}</div>
          </div>
          <div :if={@avatars != [] || @category} data-part="author-meta">
            <div :if={@avatars != []} data-part="author-avatars">
              <img :for={avatar <- @avatars} data-part="author-avatar" src={avatar} />
            </div>
            <div :if={@category} data-part="category">{@category}</div>
          </div>
        </footer>
      </body>
    </html>
    """
  end

  @doc "Render the full OG card HTML for a page."
  def render_html(opts) do
    fonts_dir = Keyword.fetch!(opts, :fonts_dir)
    logo_path = Keyword.fetch!(opts, :logo_path)

    font_base64 = fonts_dir |> Path.join("InterVariable.woff2") |> File.read!() |> Base.encode64()
    logo_base64 = logo_path |> File.read!() |> Base.encode64()

    assigns = %{
      title: Keyword.fetch!(opts, :title),
      description: Keyword.get(opts, :description),
      category: Keyword.get(opts, :category, "Docs"),
      subtitle: Keyword.get(opts, :subtitle, "Docs"),
      avatars: Keyword.get(opts, :avatars, []),
      font_data_uri: "data:font/woff2;base64,#{font_base64}",
      logo_data_uri: "data:image/png;base64,#{logo_base64}",
      image_width: @width,
      image_height: @height,
      max_title_length: @max_title_length,
      max_description_length: @max_description_length
    }

    "<!DOCTYPE html>" <>
      (assigns |> card() |> Safe.to_iodata() |> IO.iodata_to_binary())
  end

  @doc "Map a docs slug to its OG image filename, e.g. `guide/why` -> `guide-why.jpg`."
  def slug_to_filename(segments) when is_list(segments) do
    case segments do
      [] -> "index.jpg"
      _ -> Enum.join(segments, "-") <> ".jpg"
    end
  end

  defp truncate(nil, _max), do: ""

  defp truncate(text, max) do
    if String.length(text) > max do
      text |> String.slice(0, max) |> String.trim_trailing() |> Kernel.<>("...")
    else
      text
    end
  end
end
