import Config

config :logger, level: :info

config :struo_site, StruoSiteWeb.Endpoint,
  cache_static_manifest: "priv/static/cache_manifest.json"

config :struo_site, StruoSiteWeb.Endpoint,
  force_ssl: [
    rewrite_on: [:x_forwarded_proto],
    exclude: [paths: ["/ready"], hosts: ["localhost", "127.0.0.1"]]
  ]
