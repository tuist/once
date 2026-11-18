defmodule StruoSite.Application do
  # See https://hexdocs.pm/elixir/Application.html
  # for more information on OTP Applications
  @moduledoc false

  use Application

  @impl true
  def start(_type, _args) do
    children = [
      StruoSiteWeb.Telemetry,
      {DNSCluster, query: Application.get_env(:struo_site, :dns_cluster_query) || :ignore},
      {Phoenix.PubSub, name: StruoSite.PubSub},
      {StruoSite.RateLimit, [clean_period: :timer.minutes(1)]},
      StruoSiteWeb.Docs.Cache,
      # Start to serve requests, typically the last entry
      StruoSiteWeb.Endpoint
    ]

    # See https://hexdocs.pm/elixir/Supervisor.html
    # for other strategies and supported options
    opts = [strategy: :one_for_one, name: StruoSite.Supervisor]
    Supervisor.start_link(children, opts)
  end

  # Tell Phoenix to update the endpoint configuration
  # whenever the application is updated.
  @impl true
  def config_change(changed, _new, removed) do
    StruoSiteWeb.Endpoint.config_change(changed, removed)
    :ok
  end
end
