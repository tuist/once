[
  import_deps: [:phoenix],
  plugins: [Phoenix.LiveView.HTMLFormatter, Quokka],
  inputs: [
    "*.{heex,ex,exs}",
    "{config,lib,test}/**/*.{heex,ex,exs}",
    "priv/*/migrations/*.{ex,exs}"
  ]
]
