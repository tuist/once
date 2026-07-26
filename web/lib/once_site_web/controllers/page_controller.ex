defmodule OnceSiteWeb.PageController do
  use OnceSiteWeb, :controller

  def home(conn, _params) do
    conn
    |> assign(:page_title, "Build once. Reuse everywhere.")
    |> assign(:meta_description, meta_description())
    |> assign(:og_image, "/images/og/home.jpg")
    |> render(:home)
  end

  defp meta_description do
    "Once gives every action explicit inputs, outputs, and environment, so results are " <>
      "content-addressed, cached, and reusable across developers, coding agents, CI, and machines."
  end
end
