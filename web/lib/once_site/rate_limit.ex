defmodule OnceSite.RateLimit do
  @moduledoc """
  ETS-backed rate limiter for the public web endpoints (see
  `OnceSiteWeb.Plugs.RateLimit`).
  """
  use Hammer, backend: :ets
end
