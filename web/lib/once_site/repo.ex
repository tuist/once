defmodule OnceSite.Repo do
  use Ecto.Repo,
    otp_app: :once_site,
    adapter: Ecto.Adapters.Postgres
end
