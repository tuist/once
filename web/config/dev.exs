import Config

alias OnceSite.Config.DevInstance

Code.require_file("dev_instance.exs", __DIR__)

# Do not include metadata nor timestamps in development logs
config :logger, :default_formatter, format: "[$level] $message\n"

# For development, we disable any cache and enable
# debugging and code reloading.
#
# The watchers configuration can be used to run external
# watchers to your application. For example, we can use it
# to bundle .js and .css sources.
config :once_site, OnceSite.Repo,
  username: "postgres",
  password: "postgres",
  hostname: "localhost",
  database: DevInstance.database_name("once_site_dev"),
  stacktrace: true,
  show_sensitive_data_on_connection_error: true,
  pool_size: 10

config :once_site, OnceSiteWeb.Endpoint,
  # Binding to loopback ipv4 address prevents access from other machines.
  # Change to `ip: {0, 0, 0, 0}` to allow access from other machines.
  http: [ip: {127, 0, 0, 1}, port: DevInstance.port(4000)],
  check_origin: false,
  code_reloader: true,
  debug_errors: true,
  secret_key_base: "xIP+ebBfEfZbAUAeURLpV6gFSg9Wom9oi0fa+/JnP+Ll1sOZPOOmoDYEHZqmOBLK",
  watchers: [
    esbuild: {Esbuild, :install_and_run, [:once_site, ~w(--sourcemap=inline --watch)]}
  ]

# Reload browser tabs when matching files change.
config :once_site, OnceSiteWeb.Endpoint,
  live_reload: [
    web_console_logger: true,
    patterns: [
      # Static assets, except user uploads
      ~r"priv/static/(?!uploads/).*\.(js|css|png|jpeg|jpg|gif|svg)$",
      # Router, Controllers, LiveViews and LiveComponents
      ~r"lib/once_site_web/router\.ex$",
      ~r"lib/once_site_web/(controllers|live|components)/.*\.(ex|heex)$"
    ]
  ]

# Enable dev routes for dashboard and mailbox
config :once_site, dev_routes: true

# Initialize plugs at runtime for faster development compilation
config :phoenix, :plug_init_mode, :runtime

# Set a higher stacktrace during development. Avoid configuring such
# in production as building large stacktraces may be expensive.
config :phoenix, :stacktrace_depth, 20

config :phoenix_live_view,
  # Include debug annotations and locations in rendered markup.
  # Changing this configuration will require mix clean and a full recompile.
  debug_heex_annotations: true,
  debug_attributes: true,
  # Enable helpful, but potentially expensive runtime checks
  enable_expensive_runtime_checks: true
