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

  pipeline :mcp do
    plug OnceSiteWeb.Plugs.RateLimit
  end

  pipeline :image do
    plug OnceSiteWeb.Plugs.RateLimit
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
    pipe_through :image

    get "/og/zero-to-once.jpg", PassportController, :og_image
  end

  scope "/" do
    pipe_through :mcp

    forward "/mcp", EMCP.Transport.StreamableHTTP, server: OnceSite.MCP.Server
  end

  scope "/", OnceSiteWeb do
    pipe_through :browser

    get "/", PageController, :home
    get "/passports/", PassportController, :legacy_index
    get "/github.com/:account/:repository", PassportController, :show
    get "/github.com/:account/:repository/integrate", PassportController, :integration
    post "/github.com/:account/:repository/integrate", PassportController, :create_integration
    post "/github.com/:account/:repository/share", PassportController, :share

    get "/changelog", ChangelogController, :index
    get "/changelog/:slug", ChangelogController, :show
    get "/blog", BlogController, :index
    get "/blog/:slug", BlogController, :show

    get "/docs-markdown/*path", DocsMarkdownController, :show

    live_session :zero_to_once do
      live "/zero-to-once/", PassportLive, :index
    end

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
