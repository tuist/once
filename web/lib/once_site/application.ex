defmodule OnceSite.Application do
  # See https://hexdocs.pm/elixir/Application.html
  # for more information on OTP Applications
  @moduledoc false

  use Application

  @impl true
  def start(_type, _args) do
    children = [
      OnceSiteWeb.Telemetry,
      OnceSite.Repo,
      {DNSCluster, query: Application.get_env(:once_site, :dns_cluster_query) || :ignore},
      {Phoenix.PubSub, name: OnceSite.PubSub},
      # Start a worker by calling: OnceSite.Worker.start_link(arg)
      # {OnceSite.Worker, arg},
      # Start to serve requests, typically the last entry
      OnceSiteWeb.Endpoint
    ]

    # See https://hexdocs.pm/elixir/Supervisor.html
    # for other strategies and supported options
    opts = [strategy: :one_for_one, name: OnceSite.Supervisor]
    Supervisor.start_link(children, opts)
  end

  # Tell Phoenix to update the endpoint configuration
  # whenever the application is updated.
  @impl true
  def config_change(changed, _new, removed) do
    OnceSiteWeb.Endpoint.config_change(changed, removed)
    :ok
  end
end
