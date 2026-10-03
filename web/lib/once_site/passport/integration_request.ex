defmodule OnceSite.Passport.IntegrationRequest do
  @moduledoc false

  use OnceSite.Schema

  import Ecto.Changeset

  schema "passport_integration_requests" do
    field(:status, Ecto.Enum, values: [:awaiting_access, :queued, :integrating, :complete])
    field(:github_installation_id, :integer)
    field(:requested_at, :utc_datetime)
    field(:access_granted_at, :utc_datetime)
    field(:queued_at, :utc_datetime)
    field(:started_at, :utc_datetime)
    field(:share_boost, :integer, default: 0)

    belongs_to(:repository, OnceSite.Passport.Repository)

    timestamps(type: :utc_datetime)
  end

  def changeset(request, attrs) do
    request
    |> cast(attrs, [
      :repository_id,
      :status,
      :github_installation_id,
      :requested_at,
      :access_granted_at,
      :queued_at,
      :started_at,
      :share_boost
    ])
    |> validate_required([:repository_id, :status, :requested_at])
    |> unique_constraint(:repository_id)
    |> unique_constraint(:github_installation_id)
  end
end
