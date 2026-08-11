defmodule OnceSite.Passport do
  @moduledoc false

  import Ecto.Query

  alias OnceSite.Passport.Graph
  alias OnceSite.Passport.IntegrationRequest
  alias OnceSite.Passport.Repository
  alias OnceSite.Passport.Scan
  alias OnceSite.Repo

  @feature_details %{
    "cache" =>
      {"Content-addressed cache", "Reuse unchanged build and test outputs across every machine."},
    "remote_execution" =>
      {"Remote execution",
       "Run declared actions in a fresh sandbox with only their required inputs."},
    "memory_scheduling" =>
      {"Memory-aware scheduling", "Keep concurrent actions inside a declared memory budget."}
  }

  def fetch_public_repository(account, repository) do
    query =
      from(repository in Repository,
        where:
          repository.public and repository.open_source and
            repository.github_account == ^String.downcase(account) and
            repository.github_repository == ^String.downcase(repository),
        preload: [
          scans: ^from(scan in Scan, order_by: [desc: scan.checked_at], limit: 1),
          graphs: ^from(graph in Graph, order_by: [desc: graph.checked_at], limit: 1),
          integration_request: []
        ]
      )

    case Repo.one(query) do
      nil -> :error
      repository -> {:ok, repository}
    end
  end

  def fetch_or_create_public_repository(account, repository) do
    case fetch_public_repository(account, repository) do
      {:ok, repository} -> {:ok, repository}
      :error -> create_indexing_repository(account, repository)
    end
  end

  def fetch_repository(account, repository) do
    case Repo.get_by(Repository,
           github_account: String.downcase(account),
           github_repository: String.downcase(repository)
         ) do
      nil -> :error
      repository -> {:ok, Repo.preload(repository, [:scans, :graphs, :integration_request])}
    end
  end

  def submit_authorized_repository(%{"full_name" => full_name} = attributes) do
    with [account, repository] <- String.split(full_name, "/", parts: 2),
         {:ok, record} <-
           %Repository{}
           |> Repository.changeset(%{
             github_account: account,
             github_repository: repository,
             github_description: attributes["description"],
             default_branch: attributes["default_branch"],
             public: not attributes["private"],
             open_source: attributes["open_source"]
           })
           |> Repo.insert(
             on_conflict:
               {:replace,
                [:github_description, :default_branch, :public, :open_source, :updated_at]},
             conflict_target: [:github_account, :github_repository]
           ) do
      {:ok, record}
    else
      _ -> :error
    end
  end

  def list_public_repositories(params \\ %{}) do
    query =
      from(repository in Repository,
        join: request in IntegrationRequest,
        on: request.repository_id == repository.id,
        where:
          repository.public and repository.open_source and
            request.status in [:awaiting_access, :queued, :integrating],
        order_by: [desc: request.share_boost, asc: request.requested_at]
      )

    case Flop.validate_and_run(query, params, for: Repository, repo: Repo) do
      {:ok, {repositories, meta}} ->
        {Repo.preload(repositories, [:scans, :integration_request]), meta}

      {:error, meta} ->
        {[], meta}
    end
  end

  defp create_indexing_repository(account, repository) do
    key = {:github_repository, String.downcase(account), String.downcase(repository)}

    with {:ok, metadata} <- github_metadata(key, account, repository),
         false <- metadata["private"],
         %{} <- metadata["license"],
         {:ok, record} <-
           %Repository{}
           |> Repository.changeset(%{
             github_account: account,
             github_repository: repository,
             github_description: metadata["description"],
             default_branch: metadata["default_branch"],
             public: true,
             open_source: true
           })
           |> Repo.insert(
             on_conflict: :nothing,
             conflict_target: [:github_account, :github_repository]
           ) do
      {:ok, %{record | scans: []}}
    else
      true -> :error
      nil -> :error
      {:error, _reason} -> :error
    end
  end

  defp github_metadata(key, account, repository) do
    case Cachex.fetch(OnceSite.Passport.Cache, key, fn _ ->
           case OnceSite.Passport.GitHubClient.repository(account, repository) do
             {:ok, %{status: 200, body: body}} -> {:commit, {:ok, body}, expire: :timer.hours(1)}
             _ -> {:ignore, {:error, :not_found}}
           end
         end) do
      {:ok, result} -> result
      {:commit, result} -> result
      {:ignore, result} -> result
      {:error, _reason} -> {:error, :cache}
    end
  end

  def page_attributes(repository) do
    scan = current_scan(repository)

    %{
      account: repository.github_account,
      repository: repository.github_repository,
      github_description: repository.github_description,
      default_branch: repository.default_branch,
      checked_at: Calendar.strftime(scan.checked_at, "%B %-d, %Y"),
      score: scan.compatibility_score,
      summary:
        Map.get(scan.details, "summary", "This repository has been checked by Zero-to-Once."),
      estimated_savings: estimated_savings(scan.estimated_weekly_hours),
      features: Enum.map(scan.features, &Map.fetch!(@feature_details, &1)),
      graph: current_graph(repository).graph,
      graph_analysis: current_graph(repository).analysis,
      versions: history(repository.scans),
      integration: integration_attributes(repository)
    }
  end

  def title(repository),
    do: "#{repository.github_account}/#{repository.github_repository} · Zero-to-Once"

  def description(repository) do
    scan = current_scan(repository)

    "#{repository.github_account}/#{repository.github_repository} is compatible with Once. " <>
      "Potential savings: #{estimated_savings(scan.estimated_weekly_hours)}."
  end

  def public_url(repository),
    do: "/github.com/#{repository.github_account}/#{repository.github_repository}"

  def request_integration(repository) do
    attrs = %{
      repository_id: repository.id,
      status: :awaiting_access,
      requested_at: DateTime.utc_now() |> DateTime.truncate(:second)
    }

    %IntegrationRequest{}
    |> IntegrationRequest.changeset(attrs)
    |> Repo.insert(on_conflict: :nothing, conflict_target: [:repository_id])
  end

  def share_project_page(repository) do
    {updated, _} =
      from(request in IntegrationRequest,
        where: request.repository_id == ^repository.id and request.share_boost < 3
      )
      |> Repo.update_all(inc: [share_boost: 1])

    if updated > 0, do: {:ok, :boosted}, else: {:ok, :maximum_boost_reached}
  end

  def grant_installation_access(repository, installation_id) do
    now = DateTime.utc_now() |> DateTime.truncate(:second)

    from(request in IntegrationRequest, where: request.repository_id == ^repository.id)
    |> Repo.update_all(
      set: [
        status: :queued,
        github_installation_id: installation_id,
        access_granted_at: now,
        queued_at: now,
        updated_at: now
      ]
    )
  end

  defp current_scan(%{scans: [scan | _]}), do: scan
  defp current_graph(%{graphs: [graph | _]}), do: graph

  defp current_graph(%{graphs: []}) do
    %{
      graph: %{
        "summary" => %{
          "actions" => 0,
          "cached_actions" => 0,
          "critical_path_ms" => 0,
          "artifact_bytes" => 0,
          "remote_executable" => 0
        },
        "nodes" => [],
        "edges" => []
      },
      analysis: %{"observed_commits" => 0, "candidates" => []}
    }
  end

  defp estimated_savings(hours),
    do: "#{Decimal.to_string(hours, :normal)} developer hours each week"

  defp history(scans) do
    Enum.map(scans, fn scan ->
      {String.slice(scan.commit_sha, 0, 7), "Compatible",
       Map.get(scan.details, "summary", "Sandbox check completed")}
    end)
  end

  defp integration_attributes(%{integration_request: nil}) do
    %{
      status: :not_requested,
      label: "Not requested",
      detail: "Join Zero-to-Once to enter the migration queue.",
      queue_position: nil,
      share_boost: 0
    }
  end

  defp integration_attributes(%{integration_request: %{status: :awaiting_access} = request}) do
    %{
      status: :awaiting_access,
      label: "Waiting for GitHub App access",
      detail: "Install the app to confirm your queue position and let Once clone the repository.",
      queue_position: queue_position(request),
      share_boost: request.share_boost
    }
  end

  defp integration_attributes(%{integration_request: %{status: :queued} = request}) do
    %{
      status: :queued,
      label: "Queued for integration",
      detail: "Once has repository access and will prepare a native project integration.",
      queue_position: queue_position(request),
      share_boost: request.share_boost
    }
  end

  defp integration_attributes(%{integration_request: %{status: :integrating} = request}) do
    %{
      status: :integrating,
      label: "Integration in progress",
      detail: "Once is inspecting the repository and preparing its native project integration.",
      queue_position: nil,
      share_boost: request.share_boost
    }
  end

  defp integration_attributes(%{integration_request: %{status: :complete} = request}) do
    %{
      status: :complete,
      label: "Integrated",
      detail: "This repository has an active Once integration.",
      queue_position: nil,
      share_boost: request.share_boost
    }
  end

  defp queue_position(request) do
    from(other in IntegrationRequest,
      where:
        other.status in [:awaiting_access, :queued] and
          (other.share_boost > ^request.share_boost or
             (other.share_boost == ^request.share_boost and
                other.requested_at <= ^request.requested_at))
    )
    |> Repo.aggregate(:count)
  end
end
