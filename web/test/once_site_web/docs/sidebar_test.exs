defmodule OnceSiteWeb.Docs.SidebarTest do
  use ExUnit.Case, async: true

  alias OnceSiteWeb.Docs.Sidebar
  alias OnceSiteWeb.Docs.Sidebar.Group
  alias OnceSiteWeb.Docs.Sidebar.Item

  test "picks the tab from the slug prefix" do
    assert Sidebar.tab_for_slug("/docs/reference/cli/exec") == :reference
    assert Sidebar.tab_for_slug("/docs/guide/why") == :guides
    assert Sidebar.tab_for_slug("/docs") == :guides
  end

  test "trees are groups of items with /docs slugs" do
    for %Group{items: items} <- Sidebar.guide_tree() ++ Sidebar.reference_tree() do
      assert Enum.all?(items, &valid_item?/1)
    end
  end

  test "item_active? matches the current slug" do
    item = %Item{label: "Why", slug: "/docs/guide/why"}
    assert Sidebar.item_active?(item, "/docs/guide/why")
    refute Sidebar.item_active?(item, "/docs/guide/other")
  end

  test "item_or_children_active? looks into nested items" do
    parent = %Item{
      label: "Apple",
      items: [%Item{label: "apple_library", slug: "/docs/reference/prelude/apple_library"}]
    }

    assert Sidebar.item_or_children_active?(parent, "/docs/reference/prelude/apple_library")
    refute Sidebar.item_or_children_active?(parent, "/docs/reference/prelude/other")
  end

  test "exposes linting guides, commands, modules, and target kinds" do
    guide_slugs =
      Sidebar.guide_tree()
      |> Enum.flat_map(&flatten_items(&1.items))
      |> Enum.map(& &1.slug)

    reference_slugs =
      Sidebar.reference_tree()
      |> Enum.flat_map(&flatten_items(&1.items))
      |> Enum.map(& &1.slug)

    assert "/docs/guide/graph/linting" in guide_slugs
    assert "/docs/reference/cli/lint" in reference_slugs
    assert "/docs/reference/modules/linting" in reference_slugs
    assert "/docs/reference/prelude/ruff_lint" in reference_slugs
  end

  defp valid_item?(%Item{url: url}) when is_binary(url), do: true
  defp valid_item?(%Item{slug: slug, items: []}), do: String.starts_with?(slug, "/docs")
  defp valid_item?(%Item{items: items}), do: Enum.all?(items, &valid_item?/1)

  defp flatten_items(items) do
    Enum.flat_map(items, fn item -> [item | flatten_items(item.items)] end)
  end
end
