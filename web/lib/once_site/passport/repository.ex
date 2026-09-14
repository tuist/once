defmodule OnceSite.Passport.Repository do
  @moduledoc false

  use OnceSite.Schema

  import Ecto.Changeset

  @derive {Flop.Schema,
           filterable: [:github_account, :github_repository],
           sortable: [:github_account, :github_repository],
           default_order: %{order_by: [:github_account, :github_repository]},
           default_limit: 3,
           max_limit: 24}
  schema "passport_repositories" do
    field(:github_account, :string)
    field(:github_repository, :string)
    field(:github_description, :string)
    field(:default_branch, :string)
    field(:public, :boolean, default: true)
    field(:open_source, :boolean, default: false)

    has_many(:scans, OnceSite.Passport.Scan, foreign_key: :repository_id)
    has_many(:graphs, OnceSite.Passport.Graph, foreign_key: :repository_id)

    has_one(:integration_request, OnceSite.Passport.IntegrationRequest,
      foreign_key: :repository_id
    )

    timestamps(type: :utc_datetime)
  end

  def changeset(repository, attrs) do
    repository
    |> cast(attrs, [
      :github_account,
      :github_repository,
      :github_description,
      :default_branch,
      :public,
      :open_source
    ])
    |> update_change(:github_account, &String.downcase/1)
    |> update_change(:github_repository, &String.downcase/1)
    |> validate_required([
      :github_account,
      :github_repository,
      :default_branch,
      :public,
      :open_source
    ])
    |> unique_constraint([:github_account, :github_repository])
  end
end
