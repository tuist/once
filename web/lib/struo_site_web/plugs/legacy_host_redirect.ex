defmodule StruoSiteWeb.Plugs.LegacyHostRedirect do
  @moduledoc """
  Permanently redirects the hosts the site used before the rename to Struo, so
  existing links keep working. `buildonce.dev` maps to `struo.run`, and the old
  static documentation host `docs.buildonce.dev` maps to `struo.run/docs`.
  """
  import Plug.Conn

  @targets %{
    "buildonce.dev" => "https://struo.run",
    "docs.buildonce.dev" => "https://struo.run/docs"
  }

  def init(opts), do: opts

  def call(%Plug.Conn{host: host} = conn, _opts) when is_map_key(@targets, host) do
    location = Map.fetch!(@targets, host) <> conn.request_path <> query_suffix(conn.query_string)

    conn
    |> put_resp_header("location", location)
    |> send_resp(301, "")
    |> halt()
  end

  def call(conn, _opts), do: conn

  defp query_suffix(""), do: ""
  defp query_suffix(query), do: "?" <> query
end
