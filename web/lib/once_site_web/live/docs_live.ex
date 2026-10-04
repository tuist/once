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
           adjacent_pages: Sidebar.adjacent_pages(current_slug),
           edit_href: edit_href(segments),
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
            Build once, then reuse the result. Start with a working project or one script,
            verify it locally, and share the cache when you are ready. No account is needed
            for your first build.
          </p>
        </section>
        <div data-part="feature-cards">
          <.link :for={card <- overview_cards()} navigate={card.href} data-part="feature-card">
            <h2>{card.title}</h2>
            <p>{card.description}</p>
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
        <nav data-part="page-navigation" aria-label="Adjacent documentation pages">
          <.link
            :if={@adjacent_pages.previous}
            navigate={@adjacent_pages.previous.slug}
            data-part="previous-page"
          >
            <span>Previous</span>
            <strong>{@adjacent_pages.previous.label}</strong>
          </.link>
          <.link
            :if={@adjacent_pages.next}
            navigate={@adjacent_pages.next.slug}
            data-part="next-page"
          >
            <span>Next</span>
            <strong>{@adjacent_pages.next.label}</strong>
          </.link>
        </nav>
        <div data-part="markdown-link">
          <span>View</span>
          <.link_button
            label="as Markdown"
            variant="primary"
            size="large"
            href={markdown_href(@current_slug)}
          />
        </div>
        <div :if={@edit_href} data-part="edit-row">
          <.link_button
            label="Edit this page"
            variant="primary"
            size="large"
            href={@edit_href}
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
        description:
          "Install Once, reuse a script result, and restore an output in a self-contained example.",
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
        description: "Discover native projects or declare targets with explicit dependencies.",
        href: "/docs/guide/graph"
      },
      %{
        title: "Infrastructure",
        description: "Share cached results with your team or move suitable work to a sandbox.",
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

  defp edit_href(["reference", "cli" | _]), do: nil
  defp edit_href(["reference", "mcp", "tools"]), do: nil
  defp edit_href(["reference", "events"]), do: nil

  defp edit_href(segments) do
    {:ok, source} = Docs.source_path(segments)
    relative = Path.relative_to(source, Docs.root())
    "https://github.com/tuist/once/edit/main/web/priv/docs/#{relative}"
  end
end
