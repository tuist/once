defmodule OnceSiteWeb.Plugs.RateLimit do
  @moduledoc """
  Per-client rate limiting for the public web endpoints.

  The site sits behind Cloudflare, so the direct `remote_ip` is a Cloudflare
  edge address. The real client IP is read from `CF-Connecting-IP` (set by
  Cloudflare), falling back to the first `X-Forwarded-For` hop and finally the
  socket address. Requests over the limit get a `429` with a `Retry-After`
  header. Disabled in the test environment.
  """
  import Plug.Conn

  @default_limit 300
  @default_scale_ms 60_000

  def init(opts), do: opts

  def call(conn, _opts) do
    if enabled?() do
      key = "web:" <> client_ip(conn)

      case OnceSite.RateLimit.hit(key, scale_ms(), limit()) do
        {:allow, _count} ->
          conn

        {:deny, retry_after_ms} ->
          conn
          |> put_resp_header("retry-after", Integer.to_string(div(retry_after_ms, 1000)))
          |> put_resp_content_type("text/plain")
          |> send_resp(429, "Too Many Requests")
          |> halt()
      end
    else
      conn
    end
  end

  @doc "The real client IP, honouring Cloudflare and proxy headers."
  def client_ip(conn) do
    case get_req_header(conn, "cf-connecting-ip") do
      [ip | _] when ip != "" -> ip
      _ -> forwarded_for(conn)
    end
  end

  defp forwarded_for(conn) do
    case get_req_header(conn, "x-forwarded-for") do
      [value | _] when value != "" ->
        value |> String.split(",") |> List.first() |> String.trim()

      _ ->
        conn.remote_ip |> :inet.ntoa() |> to_string()
    end
  end

  defp config, do: Application.get_env(:once_site, __MODULE__, [])
  defp enabled?, do: Keyword.get(config(), :enabled, true)
  defp limit, do: Keyword.get(config(), :limit, @default_limit)
  defp scale_ms, do: Keyword.get(config(), :scale_ms, @default_scale_ms)
end
