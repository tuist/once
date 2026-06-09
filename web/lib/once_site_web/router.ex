defmodule OnceSiteWeb.Router do
  use OnceSiteWeb, :router

  pipeline :browser do
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

  scope "/", OnceSiteWeb do
    pipe_through :browser

    get "/", PageController, :home
    get "/registry", PageController, :registry
  end

  # Other scopes may use custom stacks.
  # scope "/api", OnceSiteWeb do
  #   pipe_through :api
  # end
end
