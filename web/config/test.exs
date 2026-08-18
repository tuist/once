import Config

alias OnceSite.Config.DevInstance

Code.require_file("dev_instance.exs", __DIR__)

# Print only warnings and errors during test
config :logger, level: :warning

config :once_site, OnceSiteWeb.Endpoint,
  http: [ip: {127, 0, 0, 1}, port: DevInstance.port(4002)],
  secret_key_base: "5BvQgMhh5mTPYCD6+1wjWRsLCHsL59huCdKmpBv9+hqF0BlhHMKoE82MjbUBzrxU",
  server: false

# Disable public rate limiting in tests.
config :once_site, OnceSiteWeb.Plugs.RateLimit, enabled: false

# Initialize plugs at runtime for faster test compilation
config :phoenix, :plug_init_mode, :runtime

# Sort query params output of verified routes for robust url comparisons
config :phoenix,
  sort_verified_routes_query_params: true

# Enable helpful, but potentially expensive runtime checks
config :phoenix_live_view,
  enable_expensive_runtime_checks: true
