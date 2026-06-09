defmodule OnceSiteWeb.PageController do
  use OnceSiteWeb, :controller

  alias OnceSite.Registry

  def home(conn, _params) do
    render(conn, :home)
  end

  def registry(conn, _params) do
    render(conn, :registry, entries: Registry.list_entries())
  end
end
