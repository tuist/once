defmodule OnceSiteWeb.DocsLive do
  @moduledoc """
  Serves the documentation: the landing page (`:index`) and every markdown page
  (`:page`). Pages are resolved and rendered by `OnceSiteWeb.Docs`; the shell,
  sidebar, and table of contents come from `OnceSiteWeb.Docs.Components`.
  """
  use Phoenix.LiveView
  use Noora

  import OnceSiteWeb.Docs.Components
  import Phoenix.HTML, only: [raw: 1]

  alias OnceSiteWeb.Docs
  alias OnceSiteWeb.Docs.OgImage
  alias OnceSiteWeb.Docs.Sidebar

  @impl true
  def mount(_params, _session, socket), do: {:ok, socket}

  @impl true
  def handle_params(_params, _uri, %{assigns: %{live_action: :index}} = socket) do
    {:noreply,
     socket
     |> assign(
       page_title: "Documentation",
       current_slug: "/docs",
       tab: :guides,
       headings: [],
       markdown: "",
       not_found: false,
       head_image: og_image_url("index.jpg")
     )}
  end

  def handle_params(params, _uri, socket) do
    segments = params["path"] || []
    current_slug = "/docs/" <> Enum.join(segments, "/")

    case Docs.get_page(segments) do
      {:ok, page} ->
        {:noreply,
         socket
         |> assign(
           page: page,
           slug_id: Enum.join(segments, "-"),
           current_slug: current_slug,
           tab: Sidebar.tab_for_slug(current_slug),
           headings: page.headings,
           markdown: page.markdown,
           page_title: page.title || "Documentation",
           not_found: false,
           head_image: og_image_url(OgImage.slug_to_filename(segments))
         )}

      :error ->
        {:noreply,
         socket
         |> assign(
           current_slug: current_slug,
           tab: Sidebar.tab_for_slug(current_slug),
           headings: [],
           markdown: "",
           page_title: "Page not found",
           not_found: true,
           head_image: og_image_url("index.jpg")
         )}
    end
  end

  @impl true
  def handle_event("copy-page-markdown", _params, %{assigns: %{markdown: markdown}} = socket)
      when is_binary(markdown) and markdown != "" do
    {:noreply, push_event(socket, "docs:copy-to-clipboard", %{text: markdown})}
  end

  def handle_event("copy-page-markdown", _params, socket), do: {:noreply, socket}

  @impl true
  def render(%{live_action: :index} = assigns) do
    ~H"""
    <.layout current_slug="/docs" tab={:guides} headings={[]} markdown="">
      <div id="docs-overview">
        <section data-part="hero">
          <h1>Once documentation</h1>
          <p>
            Once makes repository automation reusable across developers, coding agents, and
            machines by giving each action explicit inputs, outputs, environment, and execution
            policy.
          </p>
        </section>
        <div data-part="feature-cards">
          <.link :for={card <- overview_cards()} navigate={card.href} data-part="feature-card">
            <div data-part="image">
              <span data-part="title">{card.title}</span>
            </div>
            <div data-part="body">
              <p>{card.description}</p>
            </div>
          </.link>
        </div>
      </div>
    </.layout>
    """
  end

  def render(%{not_found: true} = assigns) do
    ~H"""
    <.layout current_slug={@current_slug} tab={@tab} headings={[]} markdown="">
      <article data-prose>
        <h1>Page not found</h1>
        <p>We could not find a documentation page at <code>{@current_slug}</code>.</p>
        <p><.link navigate="/docs">Back to the documentation home</.link></p>
      </article>
    </.layout>
    """
  end

  def render(assigns) do
    ~H"""
    <.layout
      current_slug={@current_slug}
      tab={@tab}
      headings={@headings}
      markdown={@markdown}
    >
      <article id={"docs-body-#{@slug_id}"} data-prose phx-hook="DocsContent">
        {raw(@page.html)}
      </article>
      <footer id="docs-page-footer">
        <div data-part="markdown-link">
          <span>View</span>
          <.link_button
            label="as Markdown"
            variant="primary"
            size="large"
            href={markdown_href(@current_slug)}
          />
        </div>
        <div data-part="edit-row">
          <.link_button
            label="Edit this page"
            variant="primary"
            size="large"
            href={edit_href(@current_slug)}
          >
            <:icon_left><.icon name="pencil" /></:icon_left>
          </.link_button>
        </div>
      </footer>
    </.layout>
    """
  end

  defp overview_cards do
    [
      %{
        title: "Getting Started",
        description: "Install Once and run your first cacheable script.",
        href: "/docs/guide/getting-started"
      },
      %{
        title: "Why Once",
        description: "Understand the model behind reusable automation.",
        href: "/docs/guide/why"
      },
      %{
        title: "Scripted Automation",
        description: "Add caching to the scripts you already trust.",
        href: "/docs/guide/scripted"
      },
      %{
        title: "Typed Graph",
        description: "Grow workflows into typed targets and capabilities.",
        href: "/docs/guide/graph"
      },
      %{
        title: "Infrastructure",
        description: "Run actions remotely on hosted sandboxes.",
        href: "/docs/guide/infrastructure"
      },
      %{
        title: "Reference",
        description: "The manifest, commands, and target kinds.",
        href: "/docs/reference"
      }
    ]
  end

  defp og_image_url(filename), do: OnceSiteWeb.Endpoint.url() <> "/docs/og/" <> filename

  defp markdown_href("/docs/" <> rest), do: "/docs-markdown/" <> rest
  defp markdown_href(_), do: "/docs-markdown"

  defp edit_href("/docs/" <> rest),
    do: "https://github.com/tuist/once/edit/main/web/priv/docs/#{rest}.md"

  defp edit_href(_), do: "https://github.com/tuist/once/tree/main/web/priv/docs"
end
