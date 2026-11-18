defmodule StruoSite.RateLimit do
  @moduledoc """
  ETS-backed rate limiter for the public web endpoints (see
  `StruoSiteWeb.Plugs.RateLimit`).
  """
  use Hammer, backend: :ets
end
