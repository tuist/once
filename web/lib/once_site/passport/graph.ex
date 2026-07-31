defmodule OnceSite.Passport.Graph do
  @moduledoc false

  use OnceSite.Schema

  import Ecto.Changeset

  schema "passport_graphs" do
    field(:commit_sha, :string)
    field(:branch, :string)
    field(:graph, :map, default: %{})
    field(:analysis, :map, default: %{})
    field(:checked_at, :utc_datetime)

    belongs_to(:repository, OnceSite.Passport.Repository)
    timestamps(type: :utc_datetime)
  end

  def changeset(graph, attrs) do
    graph
    |> cast(attrs, [:repository_id, :commit_sha, :branch, :graph, :analysis, :checked_at])
    |> validate_required([:repository_id, :commit_sha, :branch, :graph, :analysis, :checked_at])
    |> unique_constraint([:repository_id, :branch, :commit_sha])
  end
end
