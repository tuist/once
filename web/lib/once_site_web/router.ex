defmodule OnceSiteWeb.Router do
  use OnceSiteWeb, :router

  pipeline :browser do
    plug OnceSiteWeb.Plugs.RateLimit
    plug :accepts, ["html"]
    plug :fetch_session
    plug :fetch_live_flash
    plug :put_root_layout, html: {OnceSiteWeb.Layouts, :root}
    plug :protect_from_forgery
    plug :put_secure_browser_headers
  end

  pipeline :api do
    plug :accepts, ["json"]
  end

  # Feeds are served to clients that may not send an HTML Accept header, so they
  # skip the `:accepts ["html"]` check but keep rate limiting.
  pipeline :feed do
    plug OnceSiteWeb.Plugs.RateLimit
  end

  scope "/", OnceSiteWeb do
    get "/ready", HealthController, :ready
  end

  scope "/", OnceSiteWeb do
    pipe_through :feed

    get "/changelog/feed.xml", ChangelogController, :rss
    get "/changelog/feed.atom", ChangelogController, :atom
    get "/blog/feed.xml", BlogController, :rss
    get "/blog/feed.atom", BlogController, :atom
  end

  scope "/", OnceSiteWeb do
    pipe_through :browser

    get "/", PageController, :home

    get "/changelog", ChangelogController, :index
    get "/changelog/:slug", ChangelogController, :show
    get "/blog", BlogController, :index
    get "/blog/:slug", BlogController, :show

    get "/docs-markdown/*path", DocsMarkdownController, :show

    live_session :docs, root_layout: {OnceSiteWeb.Docs.Layouts, :root} do
      live "/docs", DocsLive, :index
      live "/docs/*path", DocsLive, :page
    end
  end

  # Other scopes may use custom stacks.
  # scope "/api", OnceSiteWeb do
  #   pipe_through :api
  # end
end
