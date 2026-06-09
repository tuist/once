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
          <span data-part="brand-mark">1</span>
          <span data-part="brand-name">Once</span>
        </a>

        <nav data-part="site-nav" aria-label="Primary navigation">
          <a href={~p"/registry"}>Registry</a>
          <a href="/guide/why">Docs</a>
          <a href="https://github.com/tuist/once">GitHub</a>
        </nav>
      </header>

      <main data-part="site-main">
        {render_slot(@inner_block)}
      </main>

      <footer data-part="site-footer">
        <span>Once</span>
        <span>Cacheable scripts, remote execution, and typed build graph foundations.</span>
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
