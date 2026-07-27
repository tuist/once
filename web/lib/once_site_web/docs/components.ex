defmodule OnceSiteWeb.Docs.Components do
  @moduledoc """
  Layout, sidebar, and table-of-contents components for the documentation,
  built from Noora components so the docs match the shared design system.
  """
  use Phoenix.Component
  use Noora

  alias OnceSiteWeb.Docs.Sidebar
  alias Phoenix.LiveView.JS

  @group_icons %{
    "Start Here" => "book_2",
    "Scripted Automation" => "file_text",
    "Typed Graph" => "subtask",
    "Infrastructure" => "server",
    "Memory" => "package",
    "Language Libraries" => "apps",
    "References" => "file_text",
    "Commands" => "devices_browser",
    "Target Kinds" => "list_tree",
    "Model Context Protocol" => "asset"
  }

  attr :current_slug, :string, required: true
  attr :tab, :atom, required: true
  attr :headings, :list, required: true
  attr :markdown, :string, required: true
  slot :inner_block, required: true

  def layout(assigns) do
    assigns = assign(assigns, :tree, Sidebar.tree_for_tab(assigns.tab))
    page_layout(assigns)
  end

  embed_templates "components/*"

  defp group_icon(label), do: Map.get(@group_icons, label, "file")

  defp docs_path(slug), do: slug

  defp docs_markdown_path("/docs/" <> rest), do: "/docs-markdown/" <> rest
  defp docs_markdown_path(_), do: "/docs-markdown"

  defp slugify(label) do
    label
    |> String.downcase()
    |> String.replace(~r/[^a-z0-9]+/, "-")
    |> String.trim("-")
  end
end
