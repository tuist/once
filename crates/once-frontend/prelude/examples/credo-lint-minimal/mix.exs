defmodule OnceLint.MixProject do
  use Mix.Project

  def project do
    [app: :once_lint, version: "0.1.0", deps: [{:credo, "~> 1.7", only: [:dev, :test]}]]
  end
end
