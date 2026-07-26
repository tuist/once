defmodule Mix.Tasks.Docs.Gen.OgImages do
  @shortdoc "Generate docs Open Graph images"

  @moduledoc """
  Generates Open Graph images for every documentation page.

  Each page's OG card (`OnceSiteWeb.Docs.OgImage`) is rendered to HTML and
  screenshotted to `priv/static/docs/og/<slug>.jpg` with headless Chrome. If
  Chrome is unavailable the task logs a warning and exits successfully, so the
  build never depends on it.
  """
  use Mix.Task

  alias OnceSiteWeb.Docs.OgImage
  alias OnceSiteWeb.Docs.OgImageRenderer
  alias OnceSiteWeb.Docs.Sidebar

  @source "priv/docs"
  @output "priv/static/docs/og"
  @fonts_dir "priv/static/fonts"
  @logo "priv/static/docs/nav-logo.png"

  @impl true
  def run(_args) do
    Mix.Task.run("app.config")
    {:ok, _} = Application.ensure_all_started(:briefly)

    case OgImageRenderer.start() do
      {:ok, renderer} ->
        try do
          generate(renderer)
        after
          OgImageRenderer.stop(renderer)
        end

      {:error, reason} ->
        Mix.shell().error(
          "[docs.gen.og_images] Chrome unavailable (#{inspect(reason)}); skipping OG image generation."
        )
    end
  end

  defp generate(renderer) do
    File.mkdir_p!(@output)
    categories = category_map()
    pages = [index_page() | markdown_pages()]

    Mix.shell().info("Generating #{length(pages)} OG images...")

    for page <- pages do
      html =
        OgImage.render_html(
          title: page.title,
          description: page.description,
          category: Map.get(categories, page.slug, "Docs"),
          fonts_dir: @fonts_dir,
          logo_path: @logo
        )

      path = Path.join(@output, page.filename)

      case OgImageRenderer.render(renderer, html) do
        {:ok, jpeg} ->
          File.write!(path, jpeg)

        {:error, reason} ->
          Mix.shell().error("  failed #{page.filename}: #{inspect(reason)}")
      end
    end
  end

  defp index_page do
    %{
      slug: "/docs",
      title: "Once Documentation",
      description: "Reusable, cacheable, remotely executable repository automation.",
      filename: "index.jpg"
    }
  end

  defp markdown_pages do
    Path.wildcard(Path.join(@source, "**/*.md"))
    |> Enum.map(fn file ->
      segments = segments(file)
      markdown = file |> File.read!() |> strip_frontmatter()

      %{
        slug: "/docs/" <> Enum.join(segments, "/"),
        title: title(markdown),
        description: description(markdown),
        filename: OgImage.slug_to_filename(segments)
      }
    end)
  end

  defp segments(file) do
    file
    |> Path.relative_to(@source)
    |> String.replace_suffix(".md", "")
    |> String.replace_suffix("/index", "")
    |> String.split("/", trim: true)
  end

  defp title(markdown) do
    case Regex.run(~r/^#\s+(.+)$/m, markdown) do
      [_, title] -> title |> String.replace("`", "") |> String.trim()
      _ -> "Once Documentation"
    end
  end

  # First prose paragraph after the title, used as the card subtitle.
  defp description(markdown) do
    markdown
    |> String.replace(~r/^#.*$/m, "")
    |> String.split("\n\n", trim: true)
    |> Enum.map(&String.trim/1)
    |> Enum.find(fn block ->
      block != "" and not String.starts_with?(block, ["```", "|", ":::", "<", "- ", "* "])
    end)
    |> case do
      nil ->
        nil

      block ->
        block
        |> String.replace(~r/[`*_>#\[\]()]/, "")
        |> String.replace(~r/\s+/, " ")
        |> String.trim()
    end
  end

  defp category_map do
    (Sidebar.guide_tree() ++ Sidebar.reference_tree())
    |> Enum.flat_map(fn group -> collect(group.label || "Docs", group.items) end)
    |> Map.new()
  end

  defp collect(category, items) do
    Enum.flat_map(items, fn item ->
      own = if item.slug, do: [{item.slug, category}], else: []
      own ++ collect(category, item.items)
    end)
  end

  defp strip_frontmatter("---\n" <> rest) do
    case String.split(rest, "\n---", parts: 2) do
      [_frontmatter, body] -> String.trim_leading(body, "\n")
      _ -> "---\n" <> rest
    end
  end

  defp strip_frontmatter(markdown), do: markdown
end
