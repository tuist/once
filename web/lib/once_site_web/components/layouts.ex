defmodule OnceSiteWeb.Layouts do
  @moduledoc """
  This module holds layouts and related functionality
  used by your application.
  """
  use OnceSiteWeb, :html

  @icons_path Path.join([File.cwd!(), "deps", "noora", "lib", "noora", "icons"])
  @github_icon @icons_path |> Path.join("brand-github.svg") |> File.read!() |> String.trim()
  @discord_icon """
  <svg width="64" height="48" viewBox="0 0 64 48" fill="none" xmlns="http://www.w3.org/2000/svg">
    <path d="M40.9051 0C40.2863 1.09866 39.7306 2.2352 39.2255 3.397C34.4268 2.67719 29.5396 2.67719 24.7283 3.397C24.2358 2.2352 23.6675 1.09866 23.0487 0C18.5404 0.770324 14.1458 2.12155 9.97847 4.02841C1.71959 16.2652 -0.51561 28.1863 0.595677 39.9432C5.4323 43.517 10.8498 46.2447 16.6209 47.9874C17.9216 46.2447 19.0708 44.3883 20.0558 42.4562C18.1868 41.7616 16.381 40.8903 14.6509 39.88C15.1055 39.5517 15.5475 39.2107 15.9769 38.8824C26.1174 43.6559 37.8617 43.6559 48.0148 38.8824C48.4441 39.236 48.8861 39.577 49.3407 39.88C47.6107 40.9029 45.8048 41.7616 43.9232 42.4688C44.9082 44.4009 46.0574 46.2573 47.3581 48C53.1292 46.2573 58.5467 43.5422 63.3834 39.9684C64.6967 26.3299 61.1355 14.5099 53.9753 4.04104C49.8206 2.13418 45.426 0.782952 40.9177 0.0252565L40.9051 0ZM21.4702 32.7072C18.351 32.7072 15.7622 29.8785 15.7622 26.3804C15.7622 22.8824 18.25 20.041 21.4576 20.041C24.6651 20.041 27.216 22.895 27.1655 26.3804C27.115 29.8658 24.6525 32.7072 21.4702 32.7072ZM42.5089 32.7072C39.3771 32.7072 36.8135 29.8785 36.8135 26.3804C36.8135 22.8824 39.3013 20.041 42.5089 20.041C45.7164 20.041 48.2547 22.895 48.2042 26.3804C48.1537 29.8658 45.6912 32.7072 42.5089 32.7072Z" fill="currentColor"/>
  </svg>
  """

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
    assigns =
      assigns
      |> assign(:github_icon, @github_icon)
      |> assign(:discord_icon, @discord_icon)

    ~H"""
    <div data-part="site-shell">
      <header data-part="site-header">
        <a href={~p"/"} data-part="brand" aria-label="Once home">
          <img data-part="brand-logo" src="/docs/nav-logo.png" alt="" />
          <span data-part="brand-name">Once</span>
        </a>

        <nav data-part="site-nav" aria-label="Primary navigation">
          <a href="/zero-to-once/">Zero-to-Once</a>
          <a href="/docs">Docs</a>
          <a href={~p"/blog"}>Blog</a>
          <a href={~p"/changelog"}>Changelog</a>
        </nav>

        <div data-part="site-actions">
          <a
            data-part="site-social-link"
            href="https://github.com/tuist/once"
            target="_blank"
            rel="noopener"
            aria-label="GitHub"
          >
            {Phoenix.HTML.raw(@github_icon)}
          </a>
          <a
            data-part="site-social-link"
            href="https://discord.gg/fTpB5e3rRp"
            target="_blank"
            rel="noopener"
            aria-label="Discord"
          >
            {Phoenix.HTML.raw(@discord_icon)}
          </a>
          <a data-part="site-cta" href="/docs/guide/getting-started">Get started</a>
        </div>
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
          <a href={~p"/blog"}>Blog</a>
          <a href={~p"/changelog"}>Changelog</a>
          <a href="https://github.com/tuist/once">GitHub</a>
          <a href="https://discord.gg/fTpB5e3rRp" target="_blank" rel="noopener">Discord</a>
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
