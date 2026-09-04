defmodule OnceSiteWeb.ZeroToOnceSession do
  @moduledoc false

  import Plug.Conn

  def session(conn) do
    case get_session(conn, :zero_to_once_repository_key) do
      nil -> %{}
      key -> %{"zero_to_once_repository_key" => key}
    end
  end
end
