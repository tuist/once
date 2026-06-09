import Config

alias OnceSite.Config.DevInstance

Code.require_file("dev_instance.exs", __DIR__)

# Print only warnings and errors during test
config :logger, level: :warning

config :once_site, OnceSite.Repo,
  username: "postgres",
  password: "postgres",
  hostname: "localhost",
  # The MIX_TEST_PARTITION environment variable can be used
  # to provide built-in test partitioning in CI.
  database:
    DevInstance.database_name(
      "once_site_test",
      partition: System.get_env("MIX_TEST_PARTITION")
    ),
  pool: Ecto.Adapters.SQL.Sandbox,
  pool_size: System.schedulers_online() * 2

# We don't run a server during test. If one is required,
# you can enable the server option below.
config :once_site, OnceSiteWeb.Endpoint,
  http: [ip: {127, 0, 0, 1}, port: DevInstance.port(4002)],
  secret_key_base: "5BvQgMhh5mTPYCD6+1wjWRsLCHsL59huCdKmpBv9+hqF0BlhHMKoE82MjbUBzrxU",
  server: false

# Initialize plugs at runtime for faster test compilation
config :phoenix, :plug_init_mode, :runtime

# Sort query params output of verified routes for robust url comparisons
config :phoenix,
  sort_verified_routes_query_params: true

# Enable helpful, but potentially expensive runtime checks
config :phoenix_live_view,
  enable_expensive_runtime_checks: true
