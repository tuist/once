defmodule Greeting.MixProject do
  use Mix.Project

  def project do
    [
      app: :greeting,
      version: "0.1.0",
      elixir: "~> 1.14",
      deps: [
        {:locked_greeting, "0.1.0"},
        {:local_helper, path: "local_helper"}
      ]
    ]
  end

  def application do
    [extra_applications: [:logger]]
  end
end
