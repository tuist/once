# This file is responsible for configuring your application
# and its dependencies with the aid of the Config module.
#
# This configuration file is loaded before any dependency and
# is restricted to this project.

# General application configuration
import Config

# Configure esbuild (the version is required)
noora_static_path = Path.expand("../deps/noora/priv/static", __DIR__)

config :esbuild,
  version: "0.25.4",
  struo_site: [
    args:
      ~w(js/app.js css/app.css --bundle --target=es2022 --outdir=../priv/static/assets --entry-names=[dir]/[name] --external:/fonts/* --external:/images/* --external:/docs/*) ++
        ["--alias:@=.", "--alias:noora/noora.css=#{noora_static_path}/noora.css"],
    cd: Path.expand("../assets", __DIR__),
    env: %{"NODE_PATH" => [Path.expand("../deps", __DIR__), Mix.Project.build_path()]}
  ],
  docs: [
    args: [
      "docs/docs.js",
      "--bundle",
      "--target=es2022",
      "--outfile=../priv/static/docs/assets/bundle.js",
      "--loader:.svg=dataurl",
      "--external:/fonts/*",
      "--external:/images/*",
      "--external:/docs/*",
      "--alias:noora=#{noora_static_path}/noora.js",
      "--alias:noora/noora.css=#{noora_static_path}/noora.css"
    ],
    cd: Path.expand("../assets", __DIR__),
    env: %{"NODE_PATH" => [Path.expand("../deps", __DIR__), Mix.Project.build_path()]}
  ]

# Configure Elixir's Logger
config :logger, :default_formatter,
  format: "$time $metadata[$level] $message\n",
  metadata: [:request_id]

config :lumis, artifact_source: :cloudflare

# Use the Lumis-enabled MDEx native highlighter so documentation code blocks are
# syntax highlighted with the github_light / github_dark themes. This pulls the
# precompiled `--lumis` NIF variant (no Rust toolchain needed).
# GitHub release headers exceed OTP's default limit; vendor mirrors retain checksums.
config :mdex_native, syntax_highlighter: :lumis, artifact_source: :cloudflare

# Import environment specific config. This must remain at the bottom
# of this file so it overrides the configuration defined above.
# Use Jason for JSON parsing in Phoenix
config :phoenix, :json_library, Jason

# Configure the endpoint
config :struo_site, StruoSiteWeb.Endpoint,
  url: [host: "localhost"],
  adapter: Bandit.PhoenixAdapter,
  render_errors: [
    formats: [html: StruoSiteWeb.ErrorHTML, json: StruoSiteWeb.ErrorJSON],
    layout: false
  ],
  pubsub_server: StruoSite.PubSub,
  live_view: [signing_salt: "TloqJLzK"]

import_config "#{config_env()}.exs"
