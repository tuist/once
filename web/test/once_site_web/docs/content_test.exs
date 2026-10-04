defmodule OnceSiteWeb.Docs.ContentTest do
  use ExUnit.Case, async: true

  alias OnceSiteWeb.Docs
  alias OnceSiteWeb.Docs.Markdown
  alias OnceSiteWeb.Docs.Sidebar

  test "sidebar pages resolve and all guides and target kinds are discoverable" do
    slugs =
      (Sidebar.guide_tree() ++ Sidebar.reference_tree())
      |> Enum.flat_map(&page_slugs(&1.items))

    assert length(slugs) == length(Enum.uniq(slugs)), "duplicate sidebar pages"

    for slug <- slugs do
      assert {:ok, _} = Docs.source_path(segments(slug)), "missing sidebar page: #{slug}"
    end

    for file <- Path.wildcard(Path.join(Docs.root(), "**/*.md")) do
      slug = page_slug(file)

      if !String.starts_with?(slug, "/docs/reference/cli/") do
        assert slug in slugs, "published page is absent from navigation: #{slug}"
      end
    end
  end

  test "documentation links resolve to pages and existing anchors" do
    pages =
      Map.new(Path.wildcard(Path.join(Docs.root(), "**/*.md")), fn file ->
        page = file |> File.read!() |> Markdown.render()
        tree = Floki.parse_fragment!(page.html)

        {file,
         %{
           links: Floki.attribute(tree, "a", "href"),
           ids: MapSet.new(Floki.attribute(tree, "[id]", "id"))
         }}
      end)

    failures =
      for {source, page} <- pages,
          href <- page.links,
          uri = URI.merge("https://docs.example.invalid" <> page_slug(source), href),
          uri.host == "docs.example.invalid",
          is_binary(uri.path),
          String.starts_with?(uri.path, "/docs/"),
          failure = link_failure(pages, uri),
          not is_nil(failure) do
        "#{Path.relative_to(source, Docs.root())}: #{failure} #{href}"
      end

    assert failures == [], Enum.join(failures, "\n")
  end

  defp link_failure(pages, uri) do
    case Docs.source_path(segments(uri.path)) do
      {:ok, destination} ->
        anchor_failure(pages[destination], uri.fragment)

      :error ->
        "missing page"
    end
  end

  defp anchor_failure(_page, fragment) when fragment in [nil, ""], do: nil

  defp anchor_failure(page, fragment) do
    if !MapSet.member?(page.ids, URI.decode(fragment)), do: "missing anchor"
  end

  defp page_slug(file) do
    "/docs/" <>
      (file
       |> Path.relative_to(Docs.root())
       |> String.replace_suffix(".md", "")
       |> String.replace_suffix("/index", ""))
  end

  defp segments("/docs/" <> path), do: String.split(path, "/", trim: true)

  defp page_slugs(items) do
    Enum.flat_map(items, fn item ->
      own = if is_binary(item.slug), do: [item.slug], else: []
      own ++ page_slugs(item.items)
    end)
  end
end
