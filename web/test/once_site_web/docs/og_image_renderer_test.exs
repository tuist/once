defmodule OnceSiteWeb.Docs.OgImageRendererTest do
  use ExUnit.Case, async: false

  alias OnceSiteWeb.Docs.OgImage
  alias OnceSiteWeb.Docs.OgImageRenderer

  @moduletag skip: System.get_env("ONCE_TEST_CHROME") != "1"

  setup_all do
    {:ok, _} = Application.ensure_all_started(:briefly)
    {:ok, renderer} = OgImageRenderer.start(1)
    on_exit(fn -> OgImageRenderer.stop(renderer) end)
    %{renderer: renderer}
  end

  test "JPEG dimensions match the social metadata across pooled renders", %{renderer: renderer} do
    for title <- ["Rust", "Build once. Reuse everywhere."] do
      {:ok, jpeg} = OgImageRenderer.render(renderer, html(title: title))
      assert jpeg_dimensions(jpeg) == {OgImage.width(), OgImage.height()}
    end
  end

  test "long text and multiple authors stay inside the card without overlapping", %{
    renderer: renderer
  } do
    logo_path = Application.app_dir(:once_site, "priv/static/docs/nav-logo.png")
    avatar = "data:image/png;base64," <> Base.encode64(File.read!(logo_path))

    cards = [
      [
        title: "Rust",
        description:
          "Once can read an existing Cargo.toml, derive a typed build graph, and cache each workspace package and locked dependency separately.",
        category: "Toolchains"
      ],
      [
        title: String.duplicate("W", 100),
        description: String.duplicate("W", 200),
        category: String.duplicate("An author, ", 20),
        subtitle: "Blog",
        avatars: [avatar, avatar]
      ],
      [title: "Changelog", description: nil, category: nil, subtitle: nil]
    ]

    for opts <- cards do
      assert {:ok, _} = OgImageRenderer.render(renderer, html(opts))

      assert {:ok, true} =
               Browse.checkout(renderer.pool, fn browser ->
                 Browse.evaluate(browser, """
                 (() => {
                   const rect = part => document.querySelector(`[data-part="${part}"]`)?.getBoundingClientRect();
                   const title = rect('title');
                   const description = rect('description');
                   const footer = rect('footer');
                   const brand = rect('brand');
                   const meta = rect('author-meta');
                   const parts = ['title', 'description', 'footer', 'brand', 'author-meta'];
                   return parts.every(part => {
                     const box = rect(part);
                     return !box || (box.left >= 0 && box.top >= 0 && box.right <= #{OgImage.width()} && box.bottom <= #{OgImage.height()});
                   }) && title.bottom < footer.top &&
                     (!description || (title.bottom < description.top && description.bottom < footer.top)) &&
                     (!meta || brand.right < meta.left) &&
                     document.fonts.check('500 128px "Inter Variable"') &&
                     Array.from(document.images).every(image => image.complete && image.naturalWidth > 0);
                 })()
                 """)
               end)
    end
  end

  defp html(opts) do
    OgImage.render_html(
      Keyword.merge(
        [
          fonts_dir: Application.app_dir(:once_site, "priv/static/fonts"),
          logo_path: Application.app_dir(:once_site, "priv/static/docs/nav-logo.png")
        ],
        opts
      )
    )
  end

  defp jpeg_dimensions(<<0xFF, 0xD8, rest::binary>>), do: jpeg_dimensions(rest)

  defp jpeg_dimensions(
         <<0xFF, marker, _length::16, _precision, height::16, width::16, _::binary>>
       )
       when marker in [0xC0, 0xC2], do: {width, height}

  defp jpeg_dimensions(<<0xFF, _marker, length::16, rest::binary>>) do
    size = length - 2
    <<_segment::binary-size(^size), rest::binary>> = rest
    jpeg_dimensions(rest)
  end
end
