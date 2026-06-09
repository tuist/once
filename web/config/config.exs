# This file is responsible for configuring your application
# and its dependencies with the aid of the Config module.
#
# This configuration file is loaded before any dependency and
# is restricted to this project.

# General application configuration
import Config

# Configure esbuild (the version is required)
config :esbuild,
  version: "0.25.4",
  once_site: [
    args:
      ~w(js/app.js css/app.css --bundle --target=es2022 --outdir=../priv/static/assets --entry-names=[dir]/[name] --external:/fonts/* --external:/images/* --alias:@=.),
    cd: Path.expand("../assets", __DIR__),
    env: %{"NODE_PATH" => [Path.expand("../deps", __DIR__), Mix.Project.build_path()]}
  ]

# Configure Elixir's Logger
config :logger, :default_formatter,
  format: "$time $metadata[$level] $message\n",
  metadata: [:request_id]

# Configure the endpoint
config :once_site, OnceSiteWeb.Endpoint,
  url: [host: "localhost"],
  adapter: Bandit.PhoenixAdapter,
  render_errors: [
    formats: [html: OnceSiteWeb.ErrorHTML, json: OnceSiteWeb.ErrorJSON],
    layout: false
  ],
  pubsub_server: OnceSite.PubSub,
  live_view: [signing_salt: "TloqJLzK"]

config :once_site,
  ecto_repos: [OnceSite.Repo],
  generators: [
    timestamp_type: :utc_datetime,
    migration_primary_key: [name: :id, type: :uuid],
    migration_foreign_key: [type: :uuid],
    binary_id_type: UUIDv7
  ]

# Import environment specific config. This must remain at the bottom

# Use Jason for JSON parsing in Phoenix
# of this file so it overrides the configuration defined above.
config :phoenix, :json_library, Jason

import_config "#{config_env()}.exs"
