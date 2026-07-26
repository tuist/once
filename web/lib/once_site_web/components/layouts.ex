defmodule OnceSiteWeb.Layouts do
  @moduledoc """
  This module holds layouts and related functionality
  used by your application.
  """
  use OnceSiteWeb, :html

  # Embed all files in layouts/* within this module.
  # The default root.html.heex file contains the HTML
  # skeleton of your application, namely HTML headers
  # and other static content.
  embed_templates "layouts/*"

  @doc "Default meta description used when a page does not set its own."
  def default_description do
    "Build once, reuse everywhere. Once makes repository automation cacheable, " <>
      "remotely executable, and reusable across developers, coding agents, and machines."
  end

  @doc """
  Renders your app layout.

  This function is typically invoked from every template,
  and it often contains your application menu, sidebar,
  or similar.

  ## Examples

      <Layouts.app flash={@flash}>
        <h1>Content</h1>
      </Layouts.app>

  """
  attr :flash, :map, required: true, doc: "the map of flash messages"

  attr :current_scope, :map,
    default: nil,
    doc: "the current [scope](https://hexdocs.pm/phoenix/scopes.html)"

  slot :inner_block, required: true

  def app(assigns) do
    ~H"""
    <div data-part="site-shell">
      <header data-part="site-header">
        <a href={~p"/"} data-part="brand" aria-label="Once home">
          <img data-part="brand-logo" src="/docs/nav-logo.png" alt="" />
          <span data-part="brand-name">Once</span>
        </a>

        <nav data-part="site-nav" aria-label="Primary navigation">
          <a href="/docs">Docs</a>
          <a href={~p"/changelog"}>Changelog</a>
          <a href="https://github.com/tuist/once">GitHub</a>
        </nav>

        <a data-part="site-cta" href="/docs/guide/getting-started">Get started</a>
      </header>

      <main data-part="site-main">
        {render_slot(@inner_block)}
      </main>

      <footer data-part="site-footer">
        <div data-part="footer-brand">
          <img data-part="brand-logo" src="/docs/nav-logo.png" alt="" />
          <span>Once</span>
        </div>
        <p>
          Build once, reuse everywhere. Built with <span data-part="heart">♥</span>
          by <a href="https://tuist.dev" target="_blank" rel="noopener">Tuist</a>.
        </p>
        <nav data-part="footer-nav" aria-label="Footer navigation">
          <a href="/docs">Documentation</a>
          <a href={~p"/changelog"}>Changelog</a>
          <a href={~p"/changelog/feed.xml"}>RSS</a>
          <a href="https://github.com/tuist/once">GitHub</a>
        </nav>
      </footer>

      <.flash_group flash={@flash} />
    </div>
    """
  end

  @doc """
  Shows the flash group with standard titles and content.

  ## Examples

      <.flash_group flash={@flash} />
  """
  attr :flash, :map, required: true, doc: "the map of flash messages"
  attr :id, :string, default: "flash-group", doc: "the optional id of flash container"

  def flash_group(assigns) do
    ~H"""
    <div id={@id} data-part="flash-group" aria-live="polite">
      <p :if={message = Phoenix.Flash.get(@flash, :info)} data-part="flash-info">{message}</p>
      <p :if={message = Phoenix.Flash.get(@flash, :error)} data-part="flash-error">{message}</p>
    </div>
    """
  end
end
