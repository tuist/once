defmodule OnceSiteWeb.Plugs.DocsRedirect do
  @moduledoc """
  Permanently redirects the legacy `docs.buildonce.dev` host to `/docs` on the
  main site, so links to the old static documentation keep working after the
  docs moved into this application.
  """
  import Plug.Conn

  @docs_host "docs.buildonce.dev"
  @target_base "https://buildonce.dev/docs"

  def init(opts), do: opts

  def call(%Plug.Conn{host: @docs_host} = conn, _opts) do
    location = @target_base <> conn.request_path <> query_suffix(conn.query_string)

    conn
    |> put_resp_header("location", location)
    |> send_resp(301, "")
    |> halt()
  end

  def call(conn, _opts), do: conn

  defp query_suffix(""), do: ""
  defp query_suffix(query), do: "?" <> query
end
