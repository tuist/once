defmodule OnceSiteWeb.Docs.OgImage do
  @moduledoc """
  Renders a documentation Open Graph card as a self-contained HTML page.

  The HTML is screenshotted to a JPG at build time by
  `Mix.Tasks.Docs.Gen.OgImages` (via headless Chrome). Fonts and the logo are
  embedded as data URIs so the page renders with no network access.
  """
  use Phoenix.Component

  @max_title_length 60
  @max_description_length 120

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
            width: 1920px;
            height: 1080px;
            overflow: hidden;
            font-family: 'Inter Variable', sans-serif;
            color-scheme: light;
            background: linear-gradient(180deg, #f4f5fe 0%, #efe8ff 100%);
          }
          .content {
            position: absolute;
            left: calc(50% - 191.5px);
            top: 50%;
            transform: translate(-50%, -50%);
            width: 1383px;
            display: flex;
            flex-direction: column;
            gap: 48px;
          }
          .title {
            font-size: 128px;
            font-weight: 500;
            letter-spacing: -6.4px;
            color: #171a1c;
            line-height: normal;
            word-wrap: break-word;
            overflow-wrap: break-word;
          }
          .description {
            font-size: 64px;
            font-weight: 500;
            letter-spacing: -3.2px;
            color: #4e575f;
            line-height: normal;
            word-wrap: break-word;
            overflow-wrap: break-word;
          }
          .logo-img { position: absolute; left: 67px; bottom: 67px; width: 80px; height: 80px; }
          .logo-once, .logo-docs {
            position: absolute;
            bottom: 67px;
            font-size: 59px;
            font-weight: 500;
            letter-spacing: -2.9px;
            line-height: 80px;
            color: #171a1c;
          }
          .logo-once { left: 175px; }
          .logo-docs { left: 340px; }
          .logo-divider {
            position: absolute;
            left: 315px;
            bottom: 67px;
            width: 3px;
            height: 80px;
            background-color: #c0c8cf;
          }
          .category {
            position: absolute;
            right: 67px;
            bottom: 67px;
            font-size: 59px;
            font-weight: 500;
            letter-spacing: -2.9px;
            color: #171a1c;
            line-height: 80px;
          }
          .author-meta {
            position: absolute;
            right: 67px;
            bottom: 67px;
            display: flex;
            align-items: center;
            gap: 24px;
          }
          .author-meta .category {
            position: static;
          }
          .author-avatars {
            display: flex;
            flex-direction: row-reverse;
          }
          .author-avatar {
            width: 80px;
            height: 80px;
            margin-right: -16px;
            border: 4px solid #f4f5fe;
            border-radius: 50%;
            background: #e4e7ec;
            object-fit: cover;
          }
          .author-avatar:last-child {
            margin-right: 0;
          }
        </style>
      </head>
      <body>
        <div class="content">
          <div class="title">{truncate(@title, @max_title_length)}</div>
          <div :if={@description} class="description">
            {truncate(@description, @max_description_length)}
          </div>
        </div>
        <img class="logo-img" src={@logo_data_uri} />
        <div class="logo-once">Once</div>
        <div :if={@subtitle} class="logo-divider"></div>
        <div :if={@subtitle} class="logo-docs">{@subtitle}</div>
        <div :if={@avatars != []} class="author-meta">
          <div class="author-avatars">
            <img :for={avatar <- @avatars} class="author-avatar" src={avatar} />
          </div>
          <div :if={@category} class="category">{@category}</div>
        </div>
        <div :if={@avatars == [] && @category} class="category">{@category}</div>
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
      max_title_length: @max_title_length,
      max_description_length: @max_description_length
    }

    "<!DOCTYPE html>" <>
      (assigns |> card() |> Phoenix.HTML.Safe.to_iodata() |> IO.iodata_to_binary())
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
