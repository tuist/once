defmodule Mix.Tasks.Marketing.Gen.OgImages do
  @shortdoc "Generate marketing Open Graph images"

  @moduledoc """
  Generates Open Graph images for the marketing pages (home, changelog).

  Cards are rendered to `priv/static/images/og/<page>.jpg` with headless Chrome.
  Like the docs task, it degrades gracefully when Chrome is unavailable.
  """
  use Mix.Task

  alias OnceSite.Changelog
  alias OnceSiteWeb.Docs.OgImage
  alias OnceSiteWeb.Docs.OgImageRenderer

  @output "priv/static/images/og"
  @fonts_dir "priv/static/fonts"
  @logo "priv/static/docs/nav-logo.png"

  @static_pages [
    %{
      file: "home.jpg",
      title: "Build once. Reuse everywhere.",
      description:
        "Cacheable, remotely executable, reusable repository automation across every language."
    },
    %{
      file: "changelog.jpg",
      title: "Changelog",
      description: "The latest user-facing changes to Once."
    }
  ]

  @impl true
  def run(_args) do
    Mix.Task.run("app.config")
    {:ok, _} = Application.ensure_all_started(:briefly)

    case OgImageRenderer.start() do
      {:ok, renderer} ->
        try do
          File.mkdir_p!(@output)

          for page <- pages() do
            html =
              OgImage.render_html(
                title: page.title,
                description: page.description,
                category: nil,
                subtitle: nil,
                fonts_dir: @fonts_dir,
                logo_path: @logo
              )

            case OgImageRenderer.render(renderer, html) do
              {:ok, jpeg} -> File.write!(Path.join(@output, page.file), jpeg)
              {:error, reason} -> Mix.shell().error("  failed #{page.file}: #{inspect(reason)}")
            end
          end

          Mix.shell().info("Generated #{length(pages())} marketing OG images.")
        after
          OgImageRenderer.stop(renderer)
        end

      {:error, reason} ->
        Mix.shell().error(
          "[marketing.gen.og_images] Chrome unavailable (#{inspect(reason)}); skipping."
        )
    end
  end

  # Static marketing pages plus one card per changelog entry (so each entry has
  # a shareable image at its detail route).
  defp pages do
    changelog_pages =
      Enum.map(Changelog.entries(), fn entry ->
        %{file: "changelog-#{entry.slug}.jpg", title: entry.title, description: entry.summary}
      end)

    @static_pages ++ changelog_pages
  end
end
