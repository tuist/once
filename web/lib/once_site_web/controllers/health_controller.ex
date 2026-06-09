defmodule OnceSiteWeb.HealthController do
  use OnceSiteWeb, :controller

  def ready(conn, _params) do
    conn
    |> put_resp_content_type("text/plain")
    |> send_resp(200, "ok")
  end
end
