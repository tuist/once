Mimic.copy(OnceSite.Registry, type_check: true)
Mimic.copy(Req, type_check: true)

ExUnit.start()
Ecto.Adapters.SQL.Sandbox.mode(OnceSite.Repo, :manual)
