defmodule OnceSiteWeb.DocsMarkdownController do
  @moduledoc """
  Serves the raw markdown source of a documentation page.

  Useful for "view as markdown" links and for tools or agents that prefer the
  source over rendered HTML. Path resolution and traversal safety are handled by
  `OnceSiteWeb.Docs`.
  """
  use OnceSiteWeb, :controller

  alias OnceSiteWeb.Docs

  def show(conn, params) do
    segments = params["path"] || []

    case Docs.source_path(segments) do
      {:ok, file} ->
        conn
        |> put_resp_content_type("text/markdown")
        |> send_file(200, file)

      :error ->
        conn
        |> put_resp_content_type("text/plain")
        |> send_resp(404, "Not found")
    end
  end
end
