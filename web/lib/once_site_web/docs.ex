defmodule OnceSiteWeb.Docs do
  @moduledoc """
  Resolves documentation URL paths to rendered pages.

  Pages live as markdown files under `priv/docs`. A URL path like
  `guide/scripted` resolves to `guide/scripted.md` or, failing that,
  `guide/scripted/index.md` (matching VitePress clean URLs). Rendering and
  caching are delegated to `OnceSiteWeb.Docs.Cache`.
  """

  alias OnceSiteWeb.Docs.Cache
  alias OnceSiteWeb.Docs.Markdown

  @doc "Return the rendered page for a list of URL path segments."
  @spec get_page([String.t()]) :: {:ok, Markdown.t()} | :error
  def get_page(segments) do
    with {:ok, file} <- resolve(segments) do
      {:ok, Cache.get(file)}
    end
  end

  @doc "Absolute path to a raw markdown file for the given segments, if it exists."
  @spec source_path([String.t()]) :: {:ok, Path.t()} | :error
  def source_path(segments), do: resolve(segments)

  @doc "Root directory holding the documentation markdown."
  @spec root() :: Path.t()
  def root, do: Application.app_dir(:once_site, "priv/docs")

  defp resolve(segments) do
    if safe?(segments) do
      slug = Enum.join(segments, "/")
      root = root()

      [Path.join(root, slug <> ".md"), Path.join([root, slug, "index.md"])]
      |> Enum.find(&File.regular?/1)
      |> case do
        nil -> :error
        file -> {:ok, file}
      end
    else
      :error
    end
  end

  # Reject empty, dot, and traversal segments so a request can never escape the
  # docs root.
  defp safe?([]), do: false

  defp safe?(segments) do
    Enum.all?(segments, fn segment ->
      segment != "" and segment != "." and segment != ".." and
        not String.contains?(segment, "/")
    end)
  end
end
